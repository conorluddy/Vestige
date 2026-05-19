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
//! The scheduler borrows an `Arc<ProjectRegistry>` so that `lib::run` can also
//! hold a reference for the IPC listener. `ProjectRegistry` is `Sync` (its inner
//! state is accessed only via `&`-ref methods), so `Arc` is safe here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Notify;
use tokio::time;

use vestige_config::ResolvedDaemonConfig;

use crate::ipc::status_file::{self, DaemonStatus, ProjectStatus};
use crate::jobs;
use crate::registry::ProjectRegistry;

// === PUBLIC API ===

/// Run the scheduler forever; return when `cancel` is notified.
///
/// The scheduler drives two periodic ticks:
///
/// - **embed tick** (`config.embed_sweep_interval_secs`): calls
///   [`jobs::embed_sweep::run_once`] for every registered project.
/// - **status tick** (hardcoded 5 s): rewrites `status_file_path` atomically
///   via [`status_file::write_atomic`].
///
/// The first embed tick is skipped (interval fires after the first period, not
/// immediately), so the daemon does not run a full sweep on every startup.
///
/// # Arguments
///
/// - `registry` — shared reference to the project registry; workers are owned
///   here and remain alive for the entire scheduler lifetime.
/// - `config` — resolved daemon configuration (used for `embed_sweep_interval_secs`).
/// - `status_file_path` — path to write the JSON status file.
/// - `started_at_rfc3339` — the daemon's start timestamp, embedded in every
///   status snapshot.
/// - `cancel` — fired by the shutdown path to stop the scheduler loop.
pub async fn run(
    registry: Arc<ProjectRegistry>,
    config: ResolvedDaemonConfig,
    status_file_path: PathBuf,
    started_at_rfc3339: String,
    cancel: Arc<Notify>,
) {
    let started = Instant::now();

    let embed_interval = std::time::Duration::from_secs(config.embed_sweep_interval_secs);
    let status_interval = std::time::Duration::from_secs(5);

    let mut embed_tick = time::interval(embed_interval);
    let mut status_tick = time::interval(status_interval);

    // Skip the first immediate embed tick so we don't run a full sweep at t=0.
    // `status_tick` fires immediately on first poll — that is intentional so the
    // status file exists as soon as the daemon is up.
    embed_tick.tick().await;

    loop {
        tokio::select! {
            _ = cancel.notified() => {
                tracing::info!("scheduler: cancellation received — stopping");
                break;
            }
            _ = embed_tick.tick() => {
                let report = jobs::embed_sweep::run_once(&registry).await;
                tracing::info!(
                    projects_scanned = report.projects_scanned,
                    projects_succeeded = report.projects_succeeded,
                    projects_failed = report.projects_failed,
                    total_embeddings_added = report.total_embeddings_added,
                    elapsed_ms = report.elapsed_ms,
                    "embed sweep finished"
                );
            }
            _ = status_tick.tick() => {
                let status = build_status(
                    &registry,
                    started,
                    &started_at_rfc3339,
                    &config,
                ).await;
                if let Err(e) = status_file::write_atomic(&status_file_path, &status) {
                    tracing::warn!(error = ?e, "status file write failed");
                }
            }
        }
    }
}

// === PRIVATE HELPERS ===

/// Assemble a [`DaemonStatus`] snapshot from the current registry state.
///
/// Pings every worker for a [`crate::workers::ProjectStatusSnapshot`] and maps
/// it to a [`ProjectStatus`]. Workers that fail to ping (e.g. thread panicked)
/// are skipped with a `warn!` log rather than aborting the whole status build.
async fn build_status(
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
