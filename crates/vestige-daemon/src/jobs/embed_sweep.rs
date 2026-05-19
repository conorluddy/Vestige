//! Embed sweep job: ask each project worker to bring its embeddings up to date.
//!
//! This module is a thin orchestration shim. The actual embedding logic lives
//! inside each worker thread via [`ProjectWorker::embed`]; this job's
//! responsibility is to fan out to every registered project, collect results,
//! and log outcomes.

use std::time::Instant;

use crate::{errors::DaemonError, registry::ProjectRegistry};

// === TYPES ===

/// Aggregate outcome of one full embed sweep across all registered projects.
#[derive(Debug, Clone)]
pub struct EmbedSweepReport {
    /// Total number of projects the sweep attempted.
    pub projects_scanned: u32,
    /// Projects for which the embed completed without error.
    pub projects_succeeded: u32,
    /// Projects for which the embed returned an error.
    pub projects_failed: u32,
    /// Sum of `embeddings_added` across all successful projects.
    pub total_embeddings_added: u64,
    /// Wall-clock duration of the sweep in milliseconds.
    pub elapsed_ms: u128,
}

// === PUBLIC API ===

/// Run one embed sweep across every project in `registry`.
///
/// Iterates [`ProjectRegistry::project_ids`], calls [`ProjectWorker::embed`] for
/// each, logs per-project outcomes, and returns the aggregated report. Errors in
/// individual projects are logged at `warn` level and counted — they do not abort
/// the sweep for the remaining projects.
pub async fn run_once(registry: &ProjectRegistry) -> EmbedSweepReport {
    let start = Instant::now();
    let mut report = EmbedSweepReport {
        projects_scanned: 0,
        projects_succeeded: 0,
        projects_failed: 0,
        total_embeddings_added: 0,
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
                "embed sweep: project disappeared from registry between id-collection and dispatch; skipping"
            );
            continue;
        };

        match worker.embed().await {
            Ok(summary) => {
                report.projects_succeeded += 1;
                report.total_embeddings_added += summary.embeddings_added;
                tracing::info!(
                    project = %project_id.as_str(),
                    representations_processed = summary.representations_processed,
                    embeddings_added = summary.embeddings_added,
                    finished_at = %summary.finished_at,
                    "embed sweep ok"
                );
            }
            Err(DaemonError::JobFailed { ref reason, .. })
                if reason == "no provider configured" =>
            {
                // Not a real failure — the daemon was started without a
                // provider.  Log at debug to avoid flooding the user.
                report.projects_failed += 1;
                tracing::debug!(
                    project = %project_id.as_str(),
                    "embed sweep skipped: no provider configured"
                );
            }
            Err(e) => {
                report.projects_failed += 1;
                tracing::warn!(
                    project = %project_id.as_str(),
                    error = %e,
                    "embed sweep failed"
                );
            }
        }
    }

    report.elapsed_ms = start.elapsed().as_millis();
    report
}
