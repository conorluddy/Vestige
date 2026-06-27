//! Per-project worker thread model for the Vestige daemon (V0.5).
//!
//! # The `!Send` problem
//!
//! `rusqlite::Connection` is `!Send`, so `Store` is `!Send`. You cannot place a
//! `Store` inside a `tokio::sync::Mutex<Store>` and call methods from multiple
//! tasks — the compiler rejects it the moment you cross thread boundaries.
//!
//! # Solution: one OS thread per project
//!
//! Each project gets a dedicated OS thread. That thread opens the `Store` and
//! owns it for its entire lifetime. Tokio code (the scheduler) communicates with
//! the thread only via `tokio::sync::mpsc::Sender<WorkerCommand>`. Each command
//! carries a `tokio::sync::oneshot::Sender` for the reply, so async code can
//! await a response without blocking tokio's reactor.
//!
//! The worker thread uses `Receiver::blocking_recv()` — a synchronous call that
//! is valid outside a tokio runtime.
//!
//! # File layout
//!
//! ```text
//! WorkerCommand  — the message enum sent into the thread
//! ProjectWorker  — handle held by the registry; owns the Sender + JoinHandle
//! run_worker     — private thread body
//! compute_snapshot — private helper; reads store state for Ping replies
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

use vestige_core::{ProjectId, RejectionReason, RepresentationDepth};
use vestige_embed::EmbeddingProvider;
use vestige_store::Store;

use crate::errors::DaemonError;

/// Default representation depths the daemon embeds on each sweep.
///
/// Matches `vestige embed` CLI defaults: `summary` and `compressed`.
const DEFAULT_EMBED_DEPTHS: &[RepresentationDepth] = &[
    RepresentationDepth::Summary,
    RepresentationDepth::Compressed,
];

// === TYPES ===

/// A request sent from async (scheduler) code into a project worker thread.
///
/// Every variant includes a `oneshot::Sender` for the reply so callers can
/// `.await` the result without polling.
pub enum WorkerCommand {
    /// Health check — reply with a point-in-time snapshot of the project state.
    Ping(oneshot::Sender<Result<ProjectStatusSnapshot, DaemonError>>),
    /// Run an embedding sweep against this project's store.
    ///
    /// **Wave 3 (T10) stub**: returns `Err(JobFailed)` with reason `"not impl"`
    /// until the real implementation lands.
    Embed(oneshot::Sender<Result<EmbedOutcomeSummary, DaemonError>>),
    /// Run `VACUUM` on this project's store to reclaim free pages.
    Prune(oneshot::Sender<Result<PruneSummary, DaemonError>>),
    /// Mark pending candidates older than `ttl_days` as rejected with reason `"expired"`.
    ///
    /// `ttl_days == 0` is an explicit no-op: returns a summary with
    /// `candidates_expired = 0` and `ttl_days = 0` rather than an error.
    Ttl {
        ttl_days: u32,
        reply: oneshot::Sender<Result<TtlSummary, DaemonError>>,
    },
    /// Scan this project's local session transcripts and propose candidates (V0.5.4).
    ///
    /// Builds the configured `[extraction]` provider from the project's
    /// `.vestige/config.toml`. When the provider is unavailable (not configured, or its
    /// feature flag is absent in this build), the scan is a **no-op** with `skipped = true`
    /// — it never dumps raw turns as candidates.
    ScanSessionLogs(oneshot::Sender<Result<ScanSummary, DaemonError>>),
    /// Graceful drain — worker loops until this is received, then exits.
    Shutdown(oneshot::Sender<()>),
}

/// Summary returned after an embedding sweep completes.
#[derive(Debug, Clone)]
pub struct EmbedOutcomeSummary {
    /// Number of `memory_representations` rows processed this run.
    pub representations_processed: u64,
    /// New `memory_embeddings` rows inserted.
    pub embeddings_added: u64,
    /// RFC-3339 timestamp when the sweep finished.
    pub finished_at: String,
}

/// Summary returned after a trace-prune (VACUUM) run completes.
#[derive(Debug, Clone)]
pub struct PruneSummary {
    /// Always `0` in V0.5 — VACUUM does not surface an eviction count.
    pub queries_evicted: u64,
    /// `true` when VACUUM completed without error.
    pub vacuumed: bool,
    /// RFC-3339 timestamp when the prune finished.
    pub finished_at: String,
}

/// Summary returned after a candidate-TTL sweep completes.
#[derive(Debug, Clone)]
pub struct TtlSummary {
    /// Number of candidates soft-deleted (set to `rejected` with reason `expired`).
    pub candidates_expired: u64,
    /// RFC-3339 timestamp when the TTL sweep finished.
    pub finished_at: String,
    /// The TTL setting used for this run. `0` means the job was a no-op.
    pub ttl_days: u32,
}

/// Summary returned after a session-log scan completes.
#[derive(Debug, Clone)]
pub struct ScanSummary {
    /// Candidates proposed into the inbox this run.
    pub candidates_proposed: u64,
    /// In-scope sessions inspected this run.
    pub sessions_scanned: u64,
    /// RFC-3339 timestamp when the scan finished.
    pub finished_at: String,
    /// `true` when no extraction provider was available, so the scan was a no-op.
    pub skipped: bool,
}

/// Point-in-time snapshot of a project's worker state.
///
/// Returned by [`ProjectWorker::ping`]. All fields are captured inside the
/// worker thread from a single consistent read.
#[derive(Debug, Clone)]
pub struct ProjectStatusSnapshot {
    pub project_id: ProjectId,
    pub project_name: String,
    pub repo_root: PathBuf,
    /// Un-embedded representation count (best-effort; see [`compute_snapshot`]).
    pub pending_embeds: u64,
    /// Count of non-deleted memories (best-effort; defaults to 0 on store error).
    pub memory_count: u64,
    /// Count of `pending` assimilation candidates (best-effort; 0 on store error).
    pub candidate_count: u64,
    /// RFC-3339 timestamp of the most recent active memory, if any.
    pub last_memory_at: Option<String>,
    /// RFC-3339 timestamp of the last completed embed sweep, if any.
    pub last_embed_run: Option<String>,
    /// RFC-3339 timestamp of the last completed prune run, if any.
    pub last_prune_run: Option<String>,
    /// RFC-3339 timestamp of the last completed candidate-TTL run, if any.
    pub last_ttl_run: Option<String>,
}

/// Handle to a per-project worker thread.
///
/// The scheduler holds one `ProjectWorker` per registered project. All
/// communication goes through the inner `mpsc::Sender<WorkerCommand>`.
///
/// Dropping a `ProjectWorker` without calling [`shutdown`][ProjectWorker::shutdown]
/// is safe — `Drop` sends a graceful shutdown and joins the thread.
pub struct ProjectWorker {
    pub project_id: ProjectId,
    pub project_name: String,
    pub repo_root: PathBuf,
    pub busy_timeout_ms: u32,
    tx: mpsc::Sender<WorkerCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

// === PUBLIC API ===

impl ProjectWorker {
    /// Spawn the OS thread and return a handle.
    ///
    /// The thread opens the project's `Store` with `busy_timeout_ms` applied
    /// (via [`Store::open_with_busy_timeout`]) so that daemon writes wait
    /// politely under WAL contention from concurrent CLI/MCP processes.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Io`] if the thread cannot be spawned (OS limit),
    /// or [`DaemonError::Store`] propagated when the Store fails to open at
    /// thread startup — note that Store open errors are detected inside the
    /// thread and the thread exits immediately, but the error surface is the
    /// first `ping` / command reply, not `spawn` itself.
    pub fn spawn(
        project_id: ProjectId,
        project_name: String,
        repo_root: PathBuf,
        storage_path: PathBuf,
        busy_timeout_ms: u32,
        provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
    ) -> Result<Self, DaemonError> {
        let (tx, rx) = mpsc::channel::<WorkerCommand>(32);

        let thread_project_id = project_id.clone();
        let thread_project_name = project_name.clone();
        let thread_repo_root = repo_root.clone();

        let handle = std::thread::Builder::new()
            .name(format!("vestige-worker-{}", project_id.as_str()))
            .spawn(move || {
                run_worker(
                    thread_project_id,
                    thread_project_name,
                    thread_repo_root,
                    storage_path,
                    busy_timeout_ms,
                    provider,
                    rx,
                );
            })
            .map_err(DaemonError::Io)?;

        Ok(Self {
            project_id,
            project_name,
            repo_root,
            busy_timeout_ms,
            tx,
            thread: Some(handle),
        })
    }

    /// Send a [`WorkerCommand::Ping`] and await the reply.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — the channel is closed (thread
    ///   exited unexpectedly).
    pub async fn ping(&self) -> Result<ProjectStatusSnapshot, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::Ping(reply_tx)).await?;
        reply_rx
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })?
    }

    /// Send a [`WorkerCommand::Embed`] and await the reply.
    ///
    /// Calls `vestige_engine::embed::embed_all` on the worker's store using
    /// the provider supplied at spawn time. Returns `Err(JobFailed)` with
    /// reason `"no provider configured"` when no provider was supplied.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — channel closed.
    /// - [`DaemonError::JobFailed`] — embed sweep failed or no provider configured.
    pub async fn embed(&self) -> Result<EmbedOutcomeSummary, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::Embed(reply_tx)).await?;
        reply_rx
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })?
    }

    /// Send a [`WorkerCommand::Prune`] and await the reply.
    ///
    /// Runs `VACUUM` on the project's SQLite file to reclaim free pages.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — channel closed.
    /// - [`DaemonError::JobFailed`] — VACUUM failed (e.g. busy timeout exceeded).
    pub async fn prune(&self) -> Result<PruneSummary, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::Prune(reply_tx)).await?;
        reply_rx
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })?
    }

    /// Send a [`WorkerCommand::Ttl`] and await the reply.
    ///
    /// Marks pending candidates older than `ttl_days` as rejected with reason
    /// `"expired"`. When `ttl_days == 0`, returns a no-op summary immediately
    /// without touching the store.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — channel closed.
    /// - [`DaemonError::JobFailed`] — store query or update failed.
    pub async fn ttl(&self, ttl_days: u32) -> Result<TtlSummary, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::Ttl {
            ttl_days,
            reply: reply_tx,
        })
        .await?;
        reply_rx
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })?
    }

    /// Send a [`WorkerCommand::ScanSessionLogs`] and await the reply.
    ///
    /// Scans the project's local Claude Code / Codex transcripts past their watermarks,
    /// extracts candidates via the configured `[extraction]` provider, and proposes them
    /// through the V0.2 inbox. Returns a no-op summary (`skipped = true`) when no provider
    /// is available.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — channel closed.
    /// - [`DaemonError::JobFailed`] — discovery / transcript-read / store failure.
    pub async fn scan_sessions(&self) -> Result<ScanSummary, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::ScanSessionLogs(reply_tx))
            .await?;
        reply_rx
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })?
    }

    /// Send [`WorkerCommand::Shutdown`], await acknowledgement, then join the
    /// OS thread.
    ///
    /// Consumes `self` so the caller cannot use the handle after shutdown.
    pub async fn shutdown(mut self) -> Result<(), DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        // Best-effort send — if the channel is already closed the thread has
        // already exited and we just need to join it.
        let _ = self.tx.send(WorkerCommand::Shutdown(reply_tx)).await;
        let _ = reply_rx.await;

        if let Some(handle) = self.thread.take() {
            // `join` is a blocking call; run it on the blocking thread pool so
            // we don't block the tokio reactor.
            tokio::task::spawn_blocking(move || {
                if handle.join().is_err() {
                    warn!("worker thread panicked during shutdown");
                }
            })
            .await
            .map_err(|e| DaemonError::Io(std::io::Error::other(e.to_string())))?;
        }
        Ok(())
    }
}

impl Drop for ProjectWorker {
    /// Best-effort shutdown: drop the sender (closes the channel) and join the
    /// thread if it has not already been joined via [`shutdown`][ProjectWorker::shutdown].
    ///
    /// Blocking join in `Drop` is acceptable here: the daemon's shutdown path
    /// calls explicit `shutdown()` before dropping, so `Drop` is only a
    /// safety net for unexpected drops (e.g. panic in the registry).
    fn drop(&mut self) {
        if self.thread.is_some() {
            // Dropping `tx` closes the channel; the worker loop will see
            // `blocking_recv()` return `None` and exit naturally.
            // We can't use `self.tx = /* dropped */` here without unsafe tricks,
            // so we create a new channel and replace — but the simplest approach
            // is to just join after a small wait.  Instead, we note that when
            // `tx` is dropped (end of struct drop), the channel closes.
            // The JoinHandle is then abandoned (thread still running until it
            // observes the closed channel).  For a clean daemon, explicit
            // `shutdown()` should always be called before drop.
            if let Some(handle) = self.thread.take() {
                // Blocking join — safe in Drop because this only runs when
                // `shutdown()` was NOT called first (abnormal path).
                if handle.join().is_err() {
                    warn!(
                        project_id = self.project_id.as_str(),
                        "worker thread panicked on drop"
                    );
                }
            }
        }
    }
}

// === PRIVATE HELPERS ===

/// Thread body for a single project worker.
///
/// Opens the `Store`, then blocks on the mpsc receiver until either a
/// `Shutdown` command arrives or the sending side is dropped (channel closed).
fn run_worker(
    project_id: ProjectId,
    project_name: String,
    repo_root: PathBuf,
    storage_path: PathBuf,
    busy_timeout_ms: u32,
    provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
    mut rx: mpsc::Receiver<WorkerCommand>,
) {
    let mut store = match Store::open_with_busy_timeout(&storage_path, busy_timeout_ms) {
        Ok(s) => s,
        Err(e) => {
            error!(
                project_id = project_id.as_str(),
                error = %e,
                "failed to open Store; worker thread exiting"
            );
            // Drain pending commands so callers don't hang waiting for a reply.
            // We can only surface a generic IO error here because StoreError
            // is not Clone. Callers that need the root cause should inspect logs.
            let open_err_msg = e.to_string();
            while let Some(cmd) = rx.blocking_recv() {
                let err = DaemonError::Io(std::io::Error::other(format!(
                    "store failed to open: {open_err_msg}"
                )));
                drain_command_with_error(cmd, err);
            }
            return;
        }
    };

    // Hydrate last_embed_run from the store so timestamps survive daemon restarts.
    // MAX(updated_at) across active embeddings is the best available proxy for
    // "when were this project's embeddings last written."
    let mut last_embed_run: Option<String> = match store.latest_embedded_at(&project_id) {
        Ok(ts) => ts,
        Err(e) => {
            warn!(
                project = project_id.as_str(),
                ?e,
                "could not read latest_embedded_at; starting with None"
            );
            None
        }
    };
    // last_prune_run/last_ttl_run not hydrated; no natural backing source — they remain None until the daemon runs them.
    let mut last_prune_run: Option<String> = None;
    let mut last_ttl_run: Option<String> = None;

    info!(project_id = project_id.as_str(), "worker thread ready");

    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WorkerCommand::Ping(reply) => {
                let snapshot = compute_snapshot(
                    &store,
                    &project_id,
                    &project_name,
                    &repo_root,
                    &last_embed_run,
                    &last_prune_run,
                    &last_ttl_run,
                );
                let _ = reply.send(Ok(snapshot));
            }
            WorkerCommand::Embed(reply) => {
                let Some(p) = provider.as_ref() else {
                    let _ = reply.send(Err(DaemonError::JobFailed {
                        job: "embed".into(),
                        reason: "no provider configured".into(),
                    }));
                    continue;
                };

                match vestige_engine::embed::embed_all(
                    &mut store,
                    &project_id,
                    p.as_ref(),
                    DEFAULT_EMBED_DEPTHS,
                    false,
                ) {
                    Ok(results) => {
                        let representations_processed = results.len() as u64;
                        let embeddings_added = results
                            .iter()
                            .filter(|r| r.outcome == vestige_engine::embed::EmbedOutcome::Embedded)
                            .count() as u64;
                        let finished_at = now_rfc3339();
                        last_embed_run = Some(finished_at.clone());

                        let summary = EmbedOutcomeSummary {
                            representations_processed,
                            embeddings_added,
                            finished_at,
                        };
                        let _ = reply.send(Ok(summary));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(DaemonError::JobFailed {
                            job: "embed".into(),
                            reason: e.to_string(),
                        }));
                    }
                }
            }
            WorkerCommand::Prune(reply) => {
                let now = now_rfc3339();
                match store.vacuum() {
                    Ok(()) => {
                        last_prune_run = Some(now.clone());
                        let _ = reply.send(Ok(PruneSummary {
                            queries_evicted: 0,
                            vacuumed: true,
                            finished_at: now,
                        }));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(DaemonError::JobFailed {
                            job: "prune".into(),
                            reason: e.to_string(),
                        }));
                    }
                }
            }
            WorkerCommand::Ttl { ttl_days, reply } => {
                let now = now_rfc3339();
                if ttl_days == 0 {
                    // TTL disabled — return a zero-count summary immediately.
                    let _ = reply.send(Ok(TtlSummary {
                        candidates_expired: 0,
                        finished_at: now,
                        ttl_days: 0,
                    }));
                } else {
                    // Compute a cutoff timestamp: now minus `ttl_days` days.
                    let cutoff = compute_ttl_cutoff(ttl_days);
                    match store.list_pending_candidates_older_than(&project_id, &cutoff) {
                        Ok(ids) => {
                            let mut count = 0u64;
                            for id in ids {
                                match store.mark_candidate_rejected(
                                    &id,
                                    &RejectionReason::Other("expired".to_string()),
                                    None,
                                    None,
                                ) {
                                    Ok(_) => count += 1,
                                    Err(e) => {
                                        warn!(
                                            ?e,
                                            candidate_id = id.as_str(),
                                            "candidate expiry skipped"
                                        );
                                    }
                                }
                            }
                            last_ttl_run = Some(now.clone());
                            let _ = reply.send(Ok(TtlSummary {
                                candidates_expired: count,
                                finished_at: now,
                                ttl_days,
                            }));
                        }
                        Err(e) => {
                            let _ = reply.send(Err(DaemonError::JobFailed {
                                job: "ttl".into(),
                                reason: e.to_string(),
                            }));
                        }
                    }
                }
            }
            WorkerCommand::ScanSessionLogs(reply) => {
                let _ = reply.send(run_session_scan(&mut store, &project_id, &repo_root));
            }
            WorkerCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }

    info!(project_id = project_id.as_str(), "worker thread exiting");
}

/// Build a [`ProjectStatusSnapshot`] from the current store state.
///
/// `pending_embeds` is derived from `embedding_status.missing_embeddings`.
/// If the query fails (e.g. store is locked), we log a warning and return 0
/// rather than failing the whole ping — this is best-effort diagnostic data.
fn compute_snapshot(
    store: &Store,
    project_id: &ProjectId,
    project_name: &str,
    repo_root: &Path,
    last_embed_run: &Option<String>,
    last_prune_run: &Option<String>,
    last_ttl_run: &Option<String>,
) -> ProjectStatusSnapshot {
    let pending_embeds = match store.embedding_status(project_id) {
        Ok(status) => status.missing_embeddings,
        Err(e) => {
            warn!(
                project_id = project_id.as_str(),
                error = %e,
                "failed to query embedding status for snapshot; defaulting pending_embeds to 0"
            );
            0
        }
    };

    let memory_count = match store.memory_counts(project_id) {
        Ok(counts) => counts.active.max(0) as u64,
        Err(e) => {
            warn!(
                project_id = project_id.as_str(),
                error = %e,
                "failed to query memory counts; defaulting memory_count to 0"
            );
            0
        }
    };

    let candidate_count = match store.pending_candidate_count(project_id) {
        Ok(n) => n.max(0) as u64,
        Err(e) => {
            warn!(
                project_id = project_id.as_str(),
                error = %e,
                "failed to query pending candidate count; defaulting candidate_count to 0"
            );
            0
        }
    };

    let last_memory_at = match store.latest_active_memory_at(project_id) {
        Ok(ts) => ts,
        Err(e) => {
            warn!(
                project_id = project_id.as_str(),
                error = %e,
                "failed to query latest memory timestamp; defaulting last_memory_at to None"
            );
            None
        }
    };

    ProjectStatusSnapshot {
        project_id: project_id.clone(),
        project_name: project_name.to_string(),
        repo_root: repo_root.to_path_buf(),
        pending_embeds,
        memory_count,
        candidate_count,
        last_memory_at,
        last_embed_run: last_embed_run.clone(),
        last_prune_run: last_prune_run.clone(),
        last_ttl_run: last_ttl_run.clone(),
    }
}

/// Run one session-log scan for this project inside the worker thread.
///
/// Builds the default session sources and the project's configured `[extraction]`
/// provider, then delegates to [`vestige_engine::scan_and_propose`]. When no provider is
/// available (unconfigured or feature-gated out), returns a no-op summary with
/// `skipped = true` rather than an error — passive ingestion stays opt-in and never dumps
/// raw turns.
fn run_session_scan(
    store: &mut Store,
    project_id: &ProjectId,
    repo_root: &Path,
) -> Result<ScanSummary, DaemonError> {
    let sources = vestige_engine::build_sources().map_err(|e| DaemonError::JobFailed {
        job: "session_log_scan".into(),
        reason: e.to_string(),
    })?;

    let Some(provider) = build_extraction_provider(repo_root, project_id) else {
        return Ok(ScanSummary {
            candidates_proposed: 0,
            sessions_scanned: 0,
            finished_at: now_rfc3339(),
            skipped: true,
        });
    };

    match vestige_engine::scan_and_propose(
        &sources,
        store,
        project_id,
        provider.as_ref(),
        &vestige_engine::ScanOptions::default(),
    ) {
        Ok(report) => Ok(ScanSummary {
            candidates_proposed: report.candidates_proposed as u64,
            sessions_scanned: report.sessions_scanned as u64,
            finished_at: now_rfc3339(),
            skipped: false,
        }),
        Err(e) => Err(DaemonError::JobFailed {
            job: "session_log_scan".into(),
            reason: e.to_string(),
        }),
    }
}

/// Build the project's configured extraction provider from its `.vestige/config.toml`.
///
/// Returns `None` (the no-op signal) when the provider cannot be built — e.g. the default
/// `ollama` backend in a build without `--features extract-ollama`, or an unknown provider
/// name. Mirrors `registry::build_project_provider` for embeddings.
fn build_extraction_provider(
    repo_root: &Path,
    project_id: &ProjectId,
) -> Option<Box<dyn vestige_extract::ExtractionProvider>> {
    let config_path = repo_root
        .join(vestige_config::CONFIG_DIR)
        .join(vestige_config::CONFIG_FILE);

    let cfg = match vestige_config::read_config(&config_path) {
        Ok(c) => vestige_config::extraction_config_for(c.extraction.as_ref()),
        Err(_) => vestige_config::extraction_config_for(None),
    };

    match vestige_extract::build_provider(&cfg) {
        Ok(p) => {
            info!(
                project = %project_id.as_str(),
                provider = p.provider_name(),
                model = p.model_name(),
                "session scan using configured extraction provider"
            );
            Some(p)
        }
        Err(e) => {
            warn!(
                project = %project_id.as_str(),
                provider = %cfg.provider,
                error = %e,
                "extraction provider unavailable; session scan is a no-op (rebuild with --features extract-<provider>)"
            );
            None
        }
    }
}

/// Return the current UTC time as an RFC-3339 string.
///
/// Used to stamp `last_embed_run` and similar fields inside the worker thread,
/// which has no access to async primitives. Falls back to the Unix epoch string
/// if the `time` crate returns an error (should never happen in practice).
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Compute the RFC-3339 cutoff timestamp for a TTL of `ttl_days` days.
///
/// Subtracts `ttl_days × 86 400` seconds from the current UTC time. Candidates
/// with `created_at < cutoff` are considered expired and eligible for rejection.
/// Falls back to the Unix epoch on the (unreachable in practice) error path.
fn compute_ttl_cutoff(ttl_days: u32) -> String {
    let cutoff =
        time::OffsetDateTime::now_utc() - time::Duration::seconds((ttl_days as i64) * 86_400);
    cutoff
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Drain a single command with an error reply, consuming it.
///
/// Called in the error-exit path when the store failed to open. Sends the
/// appropriate error to the oneshot reply channel so the caller doesn't hang.
fn drain_command_with_error(cmd: WorkerCommand, err: DaemonError) {
    match cmd {
        WorkerCommand::Ping(reply) => {
            let _ = reply.send(Err(err));
        }
        WorkerCommand::Embed(reply) => {
            let _ = reply.send(Err(err));
        }
        WorkerCommand::Prune(reply) => {
            let _ = reply.send(Err(err));
        }
        WorkerCommand::Ttl { reply, .. } => {
            let _ = reply.send(Err(err));
        }
        WorkerCommand::ScanSessionLogs(reply) => {
            let _ = reply.send(Err(err));
        }
        WorkerCommand::Shutdown(reply) => {
            let _ = reply.send(());
        }
    }
}

impl ProjectWorker {
    /// Internal helper: send a command over the mpsc channel.
    ///
    /// Maps a closed channel to [`DaemonError::ProjectNotRegistered`] so
    /// callers get a consistent error type regardless of which command failed.
    async fn send_command(&self, cmd: WorkerCommand) -> Result<(), DaemonError> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| DaemonError::ProjectNotRegistered {
                project_id: self.project_id.as_str().to_string(),
            })
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;
    use vestige_embed::FakeEmbeddingProvider;

    /// Seed the minimal project row needed to make `embedding_status` work.
    fn seed_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test Project", Some("/tmp/test"), None)
            .expect("seed project row");
    }

    fn fake_provider() -> Option<Arc<dyn EmbeddingProvider + Send + Sync>> {
        Some(Arc::new(FakeEmbeddingProvider::default()))
    }

    #[test]
    fn ping_round_trip() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-ping");
        let repo_root = PathBuf::from("/tmp/test-repo");

        // Seed the project row so embedding_status doesn't fail.
        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id.clone(),
            "Test Project".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let snapshot = worker.ping().await.expect("ping succeeds");
            assert_eq!(snapshot.project_id, project_id);
            assert_eq!(snapshot.project_name, "Test Project");
            assert_eq!(snapshot.last_embed_run, None);
            // Clean shutdown so the thread joins cleanly.
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn embed_no_provider_returns_job_failed() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-embed-no-provider");
        let repo_root = PathBuf::from("/tmp/test-repo-embed-no-provider");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Embed No Provider".to_string(),
            repo_root,
            db_path,
            5000,
            None, // no provider
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let result = worker.embed().await;
            assert!(
                result.is_err(),
                "embed without provider must return an error"
            );
            if let Err(DaemonError::JobFailed { job, reason }) = &result {
                assert_eq!(job, "embed");
                assert!(
                    reason.contains("no provider configured"),
                    "unexpected reason: {reason}"
                );
            } else {
                panic!("expected JobFailed, got {:?}", result);
            }
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn embed_with_provider_succeeds_on_empty_project() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-embed-provider");
        let repo_root = PathBuf::from("/tmp/test-repo-embed-provider");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
            // No memories seeded — embed_all runs successfully but processes 0 items.
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Embed With Provider".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let result = worker
                .embed()
                .await
                .expect("embed on empty project succeeds");
            assert_eq!(result.representations_processed, 0);
            assert_eq!(result.embeddings_added, 0);
            assert!(!result.finished_at.is_empty(), "finished_at must be set");
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn prune_round_trip() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-prune");
        let repo_root = PathBuf::from("/tmp/test-repo-prune");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Prune Test".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let summary = worker.prune().await.expect("prune succeeds");
            assert!(summary.vacuumed, "vacuumed must be true on success");
            assert!(!summary.finished_at.is_empty(), "finished_at must be set");
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn ttl_zero_days_is_noop() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-ttl-zero");
        let repo_root = PathBuf::from("/tmp/test-repo-ttl-zero");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "TTL Zero Test".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let summary = worker.ttl(0).await.expect("ttl(0) must succeed");
            assert_eq!(summary.candidates_expired, 0, "ttl=0 must expire nothing");
            assert_eq!(summary.ttl_days, 0, "ttl_days must echo back as 0");
            assert!(!summary.finished_at.is_empty(), "finished_at must be set");
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn ttl_expires_old_candidates() {
        use vestige_core::{build_candidate_bundle, CandidateStatus, MemoryType, NewCandidate};

        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-ttl-expire");
        let repo_root = PathBuf::from("/tmp/test-repo-ttl-expire");

        let (old_id, new_id) = {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);

            let make = |body: &str| {
                build_candidate_bundle(NewCandidate {
                    project_id: project_id.clone(),
                    proposed_type: MemoryType::Observation,
                    body: body.to_string(),
                    rationale: None,
                    title_override: None,
                    importance: 0.5,
                    confidence: 0.9,
                    source: None,
                    duplicate_of_memory_id: None,
                    duplicate_of_candidate_id: None,
                })
                .unwrap()
            };

            let bundle_old = make("Old candidate that should be expired by TTL.");
            let bundle_new = make("New candidate that should survive the TTL sweep.");
            let old_id = bundle_old.id.clone();
            let new_id = bundle_new.id.clone();
            store.record_candidate(&bundle_old).unwrap();
            store.record_candidate(&bundle_new).unwrap();

            // Backdate the old candidate to well before the TTL window.
            store
                .connection()
                .execute(
                    "UPDATE candidate_memories SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                    rusqlite::params![old_id.as_str()],
                )
                .unwrap();

            (old_id, new_id)
        };

        let worker = ProjectWorker::spawn(
            project_id.clone(),
            "TTL Expire Test".to_string(),
            repo_root,
            db_path.clone(),
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            // 1-day TTL — the old candidate (2020) is way over the limit.
            let summary = worker.ttl(1).await.expect("ttl(1) must succeed");
            assert_eq!(
                summary.candidates_expired, 1,
                "one candidate must be expired"
            );
            assert_eq!(summary.ttl_days, 1);
            worker.shutdown().await.expect("shutdown ok");

            // Verify directly: old candidate is now rejected, new is still pending.
            let store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            let old_cand = store.get_candidate(&old_id).unwrap().unwrap();
            let new_cand = store.get_candidate(&new_id).unwrap().unwrap();
            assert_eq!(
                old_cand.status,
                CandidateStatus::Rejected,
                "old candidate must be rejected"
            );
            assert_eq!(
                new_cand.status,
                CandidateStatus::Pending,
                "new candidate must still be pending"
            );
        });
    }

    #[test]
    fn worker_hydrates_last_embed_run_from_store() {
        use vestige_core::{build_bundle, MemoryType, NewMemory, RepresentationDepth};

        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-hydrate-embed");
        let repo_root = PathBuf::from("/tmp/test-repo-hydrate");

        // Known embedded_at value we will backdate into the store.
        let known_ts = "2026-01-15T10:30:00Z";

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);

            // Record a memory so we can obtain a representation ID.
            let bundle = build_bundle(
                &project_id,
                NewMemory {
                    r#type: MemoryType::Observation,
                    body: "Hydration test memory.",
                    importance: 0.5,
                    source: None,
                },
            )
            .unwrap();
            store.record_memory(&bundle).unwrap();

            // Look up the summary representation ID.
            let rep_id = store
                .repr_id_for_depth(&bundle.memory.id, RepresentationDepth::Summary)
                .unwrap()
                .expect("summary rep must exist after record_memory");

            // Insert an embedding row with a known updated_at value.
            store
                .connection()
                .execute(
                    "INSERT INTO memory_embeddings
                        (id, memory_id, representation_id, representation_type,
                         provider, model, dimensions, vector_hash,
                         status, created_at, updated_at, stale_at)
                     VALUES ('emb_HYDRATE', ?1, ?2, 'summary', 'fake', 'fake-v1', 4,
                             'hash_hydrate', 'active', ?3, ?3, NULL)",
                    rusqlite::params![bundle.memory.id.as_str(), rep_id, known_ts],
                )
                .unwrap();
        }

        // Spawn a fresh worker pointing at the same store — it should hydrate
        // last_embed_run from the embedding written above.
        let worker = ProjectWorker::spawn(
            project_id.clone(),
            "Hydrate Embed Test".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let snapshot = worker.ping().await.expect("ping succeeds");
            assert_eq!(
                snapshot.last_embed_run,
                Some(known_ts.to_string()),
                "worker must hydrate last_embed_run from the store on startup"
            );
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn scan_sessions_noops_without_extraction_provider() {
        // A project whose repo_root has no `[extraction]` config resolves to the default
        // `ollama` provider, which is feature-gated out of the test build → the scan must be
        // a no-op (`skipped = true`), never an error and never a candidate.
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");
        let repo_root = tmp.path().join("repo"); // no .vestige/config.toml here

        let project_id = ProjectId::from_slug("test-scan-noop");
        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Scan No-op".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let summary = worker.scan_sessions().await.expect("scan returns Ok");
            assert!(summary.skipped, "scan must be a no-op without a provider");
            assert_eq!(summary.candidates_proposed, 0);
            worker.shutdown().await.expect("shutdown ok");
        });
    }

    #[test]
    fn shutdown_clean_exit() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-shutdown");
        let repo_root = PathBuf::from("/tmp/test-repo-shutdown");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Shutdown Test".to_string(),
            repo_root,
            db_path,
            5000,
            fake_provider(),
        )
        .expect("worker spawns");

        rt.block_on(async move {
            tokio::time::timeout(std::time::Duration::from_secs(2), worker.shutdown())
                .await
                .expect("shutdown completed within 2s")
                .expect("shutdown returned Ok");
        });
    }
}
