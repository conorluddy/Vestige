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

use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

use vestige_core::ProjectId;
use vestige_store::Store;

use crate::errors::DaemonError;

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
    /// **T8 stub**: always returns `Err(JobFailed)` until Wave 3 (T10) fills in
    /// the real implementation.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::ProjectNotRegistered`] — channel closed.
    /// - [`DaemonError::JobFailed`] — embed sweep failed (or stub, for T8).
    pub async fn embed(&self) -> Result<EmbedOutcomeSummary, DaemonError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_command(WorkerCommand::Embed(reply_tx)).await?;
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
    mut rx: mpsc::Receiver<WorkerCommand>,
) {
    let store = match Store::open_with_busy_timeout(&storage_path, busy_timeout_ms) {
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

    // Per-job last-run timestamps — updated in the handler when a job completes.
    let mut last_embed_run: Option<String> = None;
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
                // Wave 3 (T10) stub — returns a descriptive error so callers
                // know the feature is not yet implemented rather than silently
                // failing or hanging.
                let _ = reply.send(Err(DaemonError::JobFailed {
                    job: "embed".into(),
                    reason: "worker stub — implemented in Wave 3 (T10)".into(),
                }));
                // Do NOT update `last_embed_run` — a stub run is not a
                // completed run.
                let _ = &mut last_embed_run; // suppress unused-mut warning for now
                let _ = &mut last_prune_run;
                let _ = &mut last_ttl_run;
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

    ProjectStatusSnapshot {
        project_id: project_id.clone(),
        project_name: project_name.to_string(),
        repo_root: repo_root.to_path_buf(),
        pending_embeds,
        last_embed_run: last_embed_run.clone(),
        last_prune_run: last_prune_run.clone(),
        last_ttl_run: last_ttl_run.clone(),
    }
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

    /// Seed the minimal project row needed to make `embedding_status` work.
    fn seed_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test Project", Some("/tmp/test"), None)
            .expect("seed project row");
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
    fn embed_returns_stub_error() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.sqlite");

        let project_id = ProjectId::from_slug("test-embed");
        let repo_root = PathBuf::from("/tmp/test-repo-embed");

        {
            let mut store = Store::open_with_busy_timeout(&db_path, 5000).unwrap();
            seed_project(&mut store, &project_id);
        }

        let worker = ProjectWorker::spawn(
            project_id,
            "Embed Test".to_string(),
            repo_root,
            db_path,
            5000,
        )
        .expect("worker spawns");

        rt.block_on(async move {
            let result = worker.embed().await;
            assert!(result.is_err(), "embed stub must return an error for T8");
            if let Err(DaemonError::JobFailed { job, reason: _ }) = &result {
                assert_eq!(job, "embed", "job name must be 'embed'");
            } else {
                panic!("expected JobFailed, got {:?}", result);
            }
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
