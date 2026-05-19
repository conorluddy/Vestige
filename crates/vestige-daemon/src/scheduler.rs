//! Periodic job dispatcher.
//!
//! Ticks each job class at its configured cadence and calls into `jobs::*::run_once`.
//! Also drives the periodic status-file refresh (every 5 seconds) so that
//! `vestige daemon status` and the Swift menu-bar app always see a fresh snapshot.
//!
//! # Cancellation
//!
//! [`run`] loops forever until the `cancel` [`tokio::sync::Notify`] fires — the
//! daemon's `wait_for_shutdown` notifier. On cancellation it returns promptly
//! without waiting for any in-flight job to finish (jobs are short-lived enough
//! that this is safe).
//!
//! # Ownership
//!
//! The scheduler takes an `Arc<tokio::sync::Mutex<ProjectRegistry>>` so that the
//! IPC server (Wave 4) can also hold a reference and call `ensure_registered`.
//! The registry lock is held only for the duration of each read/mutation — never
//! across an `await` point in the embed tick, so contention with the IPC server
//! is minimal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{watch, Mutex};
use tokio::time;

use vestige_config::ResolvedDaemonConfig;

use crate::ipc::status_file::{self, DaemonStatus, ProjectStatus};
use crate::jobs;
use crate::registry::ProjectRegistry;

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
/// # Arguments
///
/// - `registry` — shared, mutex-guarded project registry; workers are owned
///   here and remain alive for the entire scheduler lifetime.
/// - `config` — resolved daemon configuration (cadences, TTL settings).
/// - `status_file_path` — path to write the JSON status file.
/// - `started_at_rfc3339` — the daemon's start timestamp, embedded in every
///   status snapshot.
/// - `cancel` — watch receiver; the scheduler exits when its value becomes `true`.
pub async fn run(
    registry: Arc<Mutex<ProjectRegistry>>,
    config: ResolvedDaemonConfig,
    status_file_path: PathBuf,
    started_at_rfc3339: String,
    mut cancel: watch::Receiver<bool>,
) {
    let started = Instant::now();

    let embed_interval = std::time::Duration::from_secs(config.embed_sweep_interval_secs);
    let prune_interval = std::time::Duration::from_secs(config.trace_prune_interval_secs);
    let ttl_interval = std::time::Duration::from_secs(config.candidate_ttl_sweep_interval_secs);
    let status_interval = std::time::Duration::from_secs(5);

    let mut embed_tick = time::interval(embed_interval);
    let mut prune_tick = time::interval(prune_interval);
    let mut ttl_tick = time::interval(ttl_interval);
    let mut status_tick = time::interval(status_interval);

    // Skip the first immediate tick for embed/prune/ttl so we don't run full
    // sweeps at t=0. `status_tick` fires immediately on first poll — intentional
    // so the status file exists as soon as the daemon is up.
    embed_tick.tick().await;
    prune_tick.tick().await;
    ttl_tick.tick().await;

    loop {
        tokio::select! {
            // Biased so the cancel check runs first when multiple arms are ready,
            // ensuring prompt exit on cancellation even if ticks are pending.
            biased;
            result = cancel.changed() => {
                // Exit on cancellation (`true`) or sender drop (Err = orphaned).
                if result.is_err() || *cancel.borrow() {
                    tracing::info!("scheduler: cancellation received — stopping");
                    break;
                }
            }
            _ = embed_tick.tick() => {
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
                    build_status(
                        &reg,
                        started,
                        &started_at_rfc3339,
                        &config,
                    ).await
                };
                if let Err(e) = status_file::write_atomic(&status_file_path, &status) {
                    tracing::warn!(error = ?e, "status file write failed");
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
/// `pub(crate)` so `lib.rs`'s [`crate::SchedulerStatusProvider`] can call it
/// when building the IPC-facing status response.
pub(crate) async fn build_status(
    registry: &ProjectRegistry,
    started: Instant,
    started_at: &str,
    _config: &ResolvedDaemonConfig,
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

    DaemonStatus {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        started_at: started_at.to_string(),
        uptime_secs: started.elapsed().as_secs(),
        projects,
        next_jobs: Vec::new(), // Wave 5+ populates with trace_prune/candidate_ttl
    }
}
