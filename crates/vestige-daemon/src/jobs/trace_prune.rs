//! Trace prune job: VACUUM each project's SQLite DB on a slow cadence.
//!
//! Each call fans out to every registered project worker, invokes
//! [`ProjectWorker::prune`] (which runs `VACUUM`), and aggregates the outcome.
//! Individual project failures are logged at `warn` level and counted but do not
//! abort the sweep for remaining projects.
//!
//! The default cadence is 24 hours (`trace_prune_interval_secs = 86400`). VACUUM
//! is a no-op on a small or unmodified DB — running it daily is safe and cheap.

use std::time::Instant;

use crate::registry::ProjectRegistry;

// === TYPES ===

/// Aggregate outcome of one trace-prune sweep across all registered projects.
#[derive(Debug, Clone)]
pub struct TracePruneReport {
    /// Total number of projects the sweep attempted.
    pub projects_scanned: u32,
    /// Projects for which VACUUM completed without error.
    pub projects_succeeded: u32,
    /// Projects for which VACUUM returned an error.
    pub projects_failed: u32,
    /// Wall-clock duration of the sweep in milliseconds.
    pub elapsed_ms: u128,
}

// === PUBLIC API ===

/// Run one trace-prune sweep across every project in `registry`.
///
/// Iterates [`ProjectRegistry::project_ids`], calls [`crate::workers::ProjectWorker::prune`]
/// for each, logs per-project outcomes, and returns the aggregated report. Errors
/// in individual projects are logged at `warn` level and counted — they do not
/// abort the sweep for the remaining projects.
pub async fn run_once(registry: &ProjectRegistry) -> TracePruneReport {
    let start = Instant::now();
    let mut report = TracePruneReport {
        projects_scanned: 0,
        projects_succeeded: 0,
        projects_failed: 0,
        elapsed_ms: 0,
    };

    // Collect IDs first to avoid holding a reference into `registry` across
    // the `.await` points inside the loop.
    let project_ids: Vec<_> = registry.project_ids().cloned().collect();

    for project_id in project_ids {
        report.projects_scanned += 1;

        let Some(worker) = registry.get(&project_id) else {
            tracing::warn!(
                project = %project_id.as_str(),
                "trace prune: project disappeared from registry between id-collection and dispatch; skipping"
            );
            continue;
        };

        match worker.prune().await {
            Ok(summary) => {
                report.projects_succeeded += 1;
                tracing::info!(
                    project = %project_id.as_str(),
                    vacuumed = summary.vacuumed,
                    finished_at = %summary.finished_at,
                    "trace prune ok"
                );
            }
            Err(e) => {
                report.projects_failed += 1;
                tracing::warn!(
                    project = %project_id.as_str(),
                    error = %e,
                    "trace prune failed"
                );
            }
        }
    }

    report.elapsed_ms = start.elapsed().as_millis();
    report
}
