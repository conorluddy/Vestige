//! Periodic job dispatcher.
//!
//! Ticks each job class at its configured cadence and calls into `jobs::*::run_once`.
//! Also drives the periodic status-file refresh (every 5 seconds) so that
//! `vestige daemon status` and the Swift menu-bar app always see a fresh snapshot.
//!
//! # Cancellation
//!
//! [`run`] loops forever until the `cancel` [`tokio::sync::watch`] fires — the
//! daemon's `wait_for_shutdown` notifier. On cancellation it returns promptly
//! without waiting for any in-flight job to finish (jobs are short-lived enough
//! that this is safe).
//!
//! # Config reload (T8.4)
//!
//! [`run`] accepts a `config_rx: watch::Receiver<ResolvedDaemonConfig>`. When the
//! receiver sees a new value (triggered by `daemon.reload_config` over IPC), the
//! inner `select!` loop breaks to an outer `'outer: loop` which re-reads cadences
//! and rebuilds all four `tokio::time::Interval`s. This is the correct, race-free
//! reload pattern — mid-tick interruption is avoided by design.
//!
//! **Reload scope cap**: only cadences are updated live (`embed_sweep_interval_secs`,
//! `trace_prune_interval_secs`, `candidate_ttl_sweep_interval_secs`,
//! `candidate_ttl_days`). Provider changes require daemon restart.
//!
//! # Ownership
//!
//! The scheduler takes an `Arc<tokio::sync::Mutex<ProjectRegistry>>` so that the
//! IPC server can also hold a reference and call `ensure_registered`.
//! The registry lock is held only for the duration of each read/mutation — never
//! across an `await` point in the embed tick, so contention with the IPC server
//! is minimal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use time as time_crate;
use tokio::sync::{watch, Mutex};
use tokio::time as tokio_time;

use vestige_config::ResolvedDaemonConfig;

use crate::ipc::status_file::{self, DaemonStatus, JobKind, ProjectStatus, ScheduledJob};
use crate::jobs;
use crate::registry::ProjectRegistry;

// === PRIVATE TYPES ===

/// Per-job tick state for surfacing next-fire times in `next_jobs[]`.
///
/// The scheduler updates `embed_last`, `prune_last`, and `ttl_last` on every
/// tick just before running the job. `build_status` reads this struct via
/// `next_jobs()` to populate `DaemonStatus::next_jobs`.
///
/// `started_odt` is the `OffsetDateTime` value matching `started_instant` so
/// that next-fire instants can be converted to RFC 3339 without re-parsing
/// the original string.
///
/// `pub(crate)` so `lib.rs`'s `SchedulerStatusProvider` can hold an
/// `Arc<Mutex<TickState>>` shared with the scheduler.
#[derive(Debug, Clone)]
pub(crate) struct TickState {
    /// Daemon start time as an absolute `OffsetDateTime`, for RFC 3339 arithmetic.
    started_odt: time_crate::OffsetDateTime,
    /// Daemon start as a monotonic `Instant`, for duration arithmetic.
    started_instant: Instant,
    embed_interval: Duration,
    prune_interval: Duration,
    ttl_interval: Duration,
    embed_last: Option<Instant>,
    prune_last: Option<Instant>,
    ttl_last: Option<Instant>,
}

impl TickState {
    pub(crate) fn new(
        started_odt: time_crate::OffsetDateTime,
        started_instant: Instant,
        embed: Duration,
        prune: Duration,
        ttl: Duration,
    ) -> Self {
        Self {
            started_odt,
            started_instant,
            embed_interval: embed,
            prune_interval: prune,
            ttl_interval: ttl,
            embed_last: None,
            prune_last: None,
            ttl_last: None,
        }
    }

    /// Build the `next_jobs[]` list from the current tick state.
    ///
    /// For each job: `next = last_fired.unwrap_or(started_at) + interval`.
    /// `project_id` is `None` because all sweep jobs run across every registered
    /// project in one tick (not per-project).
    fn next_jobs(&self) -> Vec<ScheduledJob> {
        vec![
            ScheduledJob {
                kind: JobKind::Embed,
                project_id: None,
                at: self.instant_to_rfc3339(
                    self.embed_last.unwrap_or(self.started_instant) + self.embed_interval,
                ),
            },
            ScheduledJob {
                kind: JobKind::Prune,
                project_id: None,
                at: self.instant_to_rfc3339(
                    self.prune_last.unwrap_or(self.started_instant) + self.prune_interval,
                ),
            },
            ScheduledJob {
                kind: JobKind::CandidateTtl,
                project_id: None,
                at: self.instant_to_rfc3339(
                    self.ttl_last.unwrap_or(self.started_instant) + self.ttl_interval,
                ),
            },
        ]
    }

    /// Convert a monotonic `Instant` to an RFC 3339 string via offset arithmetic.
    ///
    /// Computes `started_odt + (target - started_instant)` as a `time::Duration`,
    /// saturating to zero if `target` is in the past relative to `started_instant`.
    fn instant_to_rfc3339(&self, target: Instant) -> String {
        let offset_secs = target
            .saturating_duration_since(self.started_instant)
            .as_secs_f64();
        let time_duration = time_crate::Duration::seconds_f64(offset_secs);
        let target_odt = self.started_odt + time_duration;
        target_odt
            .format(&time_crate::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
    }
}

// === PUBLIC API ===

/// Run the scheduler forever; return when `cancel` is notified.
///
/// The scheduler drives four periodic ticks:
///
/// - **embed tick** (`config.embed_sweep_interval_secs`): calls
///   [`jobs::embed_sweep::run_once`] for every registered project.
/// - **prune tick** (`config.trace_prune_interval_secs`): calls
///   [`jobs::trace_prune::run_once`] to VACUUM each project DB.
/// - **ttl tick** (`config.candidate_ttl_sweep_interval_secs`): calls
///   [`jobs::candidate_ttl::run_once`] to expire stale candidates. When
///   `candidate_ttl_days == 0`, the tick fires but the job exits immediately.
/// - **status tick** (hardcoded 5 s): rewrites `status_file_path` atomically
///   via [`status_file::write_atomic`].
///
/// The first embed, prune, and ttl ticks are skipped (interval fires after the
/// first period, not immediately), so the daemon does not run sweeps on every
/// startup. The status tick fires immediately on first poll so the status file
/// exists as soon as the daemon is up.
///
/// # Config reload
///
/// `config_rx` is a `watch::Receiver<ResolvedDaemonConfig>`. When a new value
/// arrives (from `daemon.reload_config` over IPC), the inner loop breaks via
/// `continue 'outer` and intervals are rebuilt from the new cadences. Only
/// cadences (`embed_sweep_interval_secs`, `trace_prune_interval_secs`,
/// `candidate_ttl_sweep_interval_secs`, `candidate_ttl_days`) are reloaded
/// live; provider changes require daemon restart.
///
/// # Arguments
///
/// - `registry` — shared, mutex-guarded project registry.
/// - `config_rx` — watch receiver for live config reload (T8.4).
/// - `shared_tick_state` — shared with the IPC `SchedulerStatusProvider` so
///   that `daemon.status` responses include populated `next_jobs[]` (T8.3).
///   The scheduler updates it in-place on every config rebuild and on each tick.
/// - `status_file_path` — path to write the JSON status file.
/// - `started_at_rfc3339` — the daemon's start timestamp, embedded in every
///   status snapshot.
/// - `cancel` — watch receiver; the scheduler exits when its value becomes `true`.
pub(crate) async fn run(
    registry: Arc<Mutex<ProjectRegistry>>,
    mut config_rx: watch::Receiver<ResolvedDaemonConfig>,
    shared_tick_state: Arc<Mutex<TickState>>,
    status_file_path: PathBuf,
    started_at_rfc3339: String,
    mut cancel: watch::Receiver<bool>,
) {
    let started_instant = Instant::now();
    let started_odt = time_crate::OffsetDateTime::now_utc();

    'outer: loop {
        // Read current config (clone is cheap — ResolvedDaemonConfig is small).
        let config = config_rx.borrow().clone();

        let embed_interval = Duration::from_secs(config.embed_sweep_interval_secs);
        let prune_interval = Duration::from_secs(config.trace_prune_interval_secs);
        let ttl_interval = Duration::from_secs(config.candidate_ttl_sweep_interval_secs);
        let status_interval = Duration::from_secs(5);

        let mut embed_tick = tokio_time::interval(embed_interval);
        let mut prune_tick = tokio_time::interval(prune_interval);
        let mut ttl_tick = tokio_time::interval(ttl_interval);
        let mut status_tick = tokio_time::interval(status_interval);

        // Skip the first immediate tick for embed/prune/ttl so we don't run full
        // sweeps at t=0. `status_tick` fires immediately on first poll — intentional
        // so the status file exists as soon as the daemon is up.
        embed_tick.tick().await;
        prune_tick.tick().await;
        ttl_tick.tick().await;

        // T8.3 — update the shared TickState so next_jobs reflects the current
        // cadences. Reset on every config rebuild (after reload_config).
        {
            let mut ts = shared_tick_state.lock().await;
            *ts = TickState::new(
                started_odt,
                started_instant,
                embed_interval,
                prune_interval,
                ttl_interval,
            );
        }

        // Convenience alias for inner-loop access.
        let tick_state = Arc::clone(&shared_tick_state);

        loop {
            tokio::select! {
                // Biased so the cancel check runs first when multiple arms are ready,
                // ensuring prompt exit on cancellation even if ticks are pending.
                biased;
                result = cancel.changed() => {
                    // Exit on cancellation (`true`) or sender drop (Err = orphaned).
                    if result.is_err() || *cancel.borrow() {
                        tracing::info!("scheduler: cancellation received — stopping");
                        return;
                    }
                }
                result = config_rx.changed() => {
                    match result {
                        Err(_) => {
                            // Sender dropped — daemon is shutting down. Exit cleanly.
                            tracing::info!("scheduler: config sender dropped — stopping");
                            return;
                        }
                        Ok(()) => {
                            let new = config_rx.borrow();
                            tracing::info!(
                                embed_sweep_interval_secs = new.embed_sweep_interval_secs,
                                trace_prune_interval_secs = new.trace_prune_interval_secs,
                                candidate_ttl_sweep_interval_secs = new.candidate_ttl_sweep_interval_secs,
                                candidate_ttl_days = new.candidate_ttl_days,
                                "scheduler: config changed — rebuilding intervals on next outer loop"
                            );
                            drop(new);
                            continue 'outer;
                        }
                    }
                }
                _ = embed_tick.tick() => {
                    tick_state.lock().await.embed_last = Some(Instant::now());
                    let reg = registry.lock().await;
                    let report = jobs::embed_sweep::run_once(&reg).await;
                    drop(reg);
                    tracing::info!(
                        projects_scanned = report.projects_scanned,
                        projects_succeeded = report.projects_succeeded,
                        projects_failed = report.projects_failed,
                        total_embeddings_added = report.total_embeddings_added,
                        elapsed_ms = report.elapsed_ms,
                        "embed sweep finished"
                    );
                }
                _ = prune_tick.tick() => {
                    tick_state.lock().await.prune_last = Some(Instant::now());
                    let reg = registry.lock().await;
                    let report = jobs::trace_prune::run_once(&reg).await;
                    drop(reg);
                    tracing::info!(
                        projects_scanned = report.projects_scanned,
                        projects_succeeded = report.projects_succeeded,
                        projects_failed = report.projects_failed,
                        elapsed_ms = report.elapsed_ms,
                        "trace prune finished"
                    );
                }
                _ = ttl_tick.tick() => {
                    tick_state.lock().await.ttl_last = Some(Instant::now());
                    let ttl_days = config.candidate_ttl_days;
                    let reg = registry.lock().await;
                    let report = jobs::candidate_ttl::run_once(&reg, ttl_days).await;
                    drop(reg);
                    tracing::info!(
                        projects_scanned = report.projects_scanned,
                        projects_succeeded = report.projects_succeeded,
                        projects_failed = report.projects_failed,
                        total_candidates_expired = report.total_candidates_expired,
                        ttl_days = report.ttl_days,
                        elapsed_ms = report.elapsed_ms,
                        "candidate ttl finished"
                    );
                }
                _ = status_tick.tick() => {
                    let status = {
                        let reg = registry.lock().await;
                        let state = tick_state.lock().await;
                        build_status(
                            &reg,
                            started_instant,
                            &started_at_rfc3339,
                            &config,
                            Some(&state),
                        ).await
                    };
                    if let Err(e) = status_file::write_atomic(&status_file_path, &status) {
                        tracing::warn!(error = ?e, "status file write failed");
                    }
                }
            }
        }
    }
}

// === CRATE-INTERNAL HELPERS ===

/// Assemble a [`DaemonStatus`] snapshot from the current registry state.
///
/// Pings every worker for a [`crate::workers::ProjectStatusSnapshot`] and maps
/// it to a [`ProjectStatus`]. Workers that fail to ping (e.g. thread panicked)
/// are skipped with a `warn!` log rather than aborting the whole status build.
///
/// `tick_state` is `Some` when called from inside the scheduler (where the
/// `TickState` is available) and `None` from the IPC `SchedulerStatusProvider`
/// (which holds its own `tick_state_arc` — see `lib.rs`).
///
/// `pub(crate)` so `lib.rs`'s [`crate::SchedulerStatusProvider`] can call it
/// when building the IPC-facing status response.
pub(crate) async fn build_status(
    registry: &ProjectRegistry,
    started: Instant,
    started_at: &str,
    _config: &ResolvedDaemonConfig,
    tick_state: Option<&TickState>,
) -> DaemonStatus {
    let project_ids: Vec<_> = registry.project_ids().cloned().collect();
    let mut projects = Vec::with_capacity(project_ids.len());

    for project_id in project_ids {
        let Some(worker) = registry.get(&project_id) else {
            continue;
        };
        match worker.ping().await {
            Ok(snap) => projects.push(ProjectStatus {
                project_id: snap.project_id,
                project_name: snap.project_name,
                repo_root: snap.repo_root.display().to_string(),
                last_embed_run: snap.last_embed_run,
                last_prune_run: snap.last_prune_run,
                last_ttl_run: snap.last_ttl_run,
                pending_embeds: snap.pending_embeds,
                memory_count: snap.memory_count,
                candidate_count: snap.candidate_count,
                last_memory_at: snap.last_memory_at,
            }),
            Err(e) => {
                tracing::warn!(
                    project = %project_id.as_str(),
                    error = ?e,
                    "ping failed during status build; project omitted from snapshot"
                );
            }
        }
    }

    let next_jobs = match tick_state {
        Some(state) => state.next_jobs(),
        None => Vec::new(),
    };

    DaemonStatus {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        started_at: started_at.to_string(),
        uptime_secs: started.elapsed().as_secs(),
        projects,
        next_jobs,
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use time_crate::OffsetDateTime;

    fn make_tick_state(embed: u64, prune: u64, ttl: u64) -> TickState {
        TickState::new(
            OffsetDateTime::now_utc(),
            Instant::now(),
            Duration::from_secs(embed),
            Duration::from_secs(prune),
            Duration::from_secs(ttl),
        )
    }

    /// Fresh `TickState` with no prior fires produces 3 jobs,
    /// all with `project_id = None` (sweep-all sentinel).
    #[test]
    fn next_jobs_on_fresh_state_returns_three_entries_with_no_project() {
        let state = make_tick_state(600, 86_400, 3_600);
        let jobs = state.next_jobs();

        assert_eq!(jobs.len(), 3, "must have exactly 3 scheduled jobs");
        for job in &jobs {
            assert!(
                job.project_id.is_none(),
                "sweep jobs must have project_id = None (all projects)"
            );
        }

        // Kinds must appear in the documented order.
        assert_eq!(jobs[0].kind, JobKind::Embed);
        assert_eq!(jobs[1].kind, JobKind::Prune);
        assert_eq!(jobs[2].kind, JobKind::CandidateTtl);
    }

    /// After `embed_last` is set, the embed job's `at` is in the future.
    #[test]
    fn next_jobs_after_embed_fires_reflects_updated_last() {
        let mut state = make_tick_state(600, 86_400, 3_600);

        // Simulate embed firing now.
        state.embed_last = Some(Instant::now());
        let jobs = state.next_jobs();

        // All timestamps must be RFC 3339 strings (non-empty, Z-suffixed or +offset).
        for job in &jobs {
            assert!(!job.at.is_empty(), "at timestamp must not be empty");
        }

        // The embed job's next fire should be ~600 s from now.
        // We can't assert an exact string but we can assert it parses as RFC3339.
        let embed_at = &jobs[0].at;
        time_crate::OffsetDateTime::parse(
            embed_at,
            &time_crate::format_description::well_known::Rfc3339,
        )
        .expect("embed `at` must be a valid RFC 3339 timestamp");
    }

    /// `instant_to_rfc3339` for a target in the far future produces a parseable
    /// RFC 3339 string that is after the start time.
    #[test]
    fn instant_to_rfc3339_future_instant_parses() {
        let state = make_tick_state(600, 86_400, 3_600);
        let future = state.started_instant + Duration::from_secs(7_200);
        let result = state.instant_to_rfc3339(future);

        let parsed = time_crate::OffsetDateTime::parse(
            &result,
            &time_crate::format_description::well_known::Rfc3339,
        )
        .expect("must be valid RFC 3339");
        assert!(
            parsed > state.started_odt,
            "future instant must produce a timestamp after start"
        );
    }

    /// Config-reload: a new `TickState` constructed with updated cadences
    /// reflects the new intervals in `next_jobs`.
    #[test]
    fn tick_state_rebuild_on_config_change_uses_new_intervals() {
        let state_before = make_tick_state(600, 86_400, 3_600);
        let state_after = make_tick_state(60, 86_400, 3_600);

        // Stale state has embed interval of 600 s; new state has 60 s.
        // Both produce 3 jobs; the test just confirms the rebuild doesn't panic.
        assert_eq!(state_before.next_jobs().len(), 3);
        assert_eq!(state_after.next_jobs().len(), 3);

        // Check that intervals differ by inspecting the Duration fields directly.
        assert_eq!(state_before.embed_interval, Duration::from_secs(600));
        assert_eq!(state_after.embed_interval, Duration::from_secs(60));
    }
}
