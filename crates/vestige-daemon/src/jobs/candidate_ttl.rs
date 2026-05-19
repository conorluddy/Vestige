//! Candidate TTL job: mark pending candidates older than `ttl_days` as rejected.
//!
//! The rejection reason stored is `"expired"` (a free-form `RejectionReason::Other`
//! value — soft-delete only, no rows are ever deleted).
//!
//! # Opt-in
//!
//! When `ttl_days == 0`, the scheduler does not even tick this job (cadence is
//! still configured, but the `run_once` guard returns immediately). Callers that
//! invoke `run_once(registry, 0)` anyway receive an empty, zero-count report.
//!
//! # Project scope
//!
//! Each worker only touches its own project's DB — the project-scope boundary is
//! enforced by the worker and the store query.

use std::time::Instant;

use crate::registry::ProjectRegistry;

// === TYPES ===

/// Aggregate outcome of one candidate-TTL sweep across all registered projects.
#[derive(Debug, Clone)]
pub struct CandidateTtlReport {
    /// Total number of projects the sweep attempted.
    pub projects_scanned: u32,
    /// Projects for which the TTL sweep completed without error.
    pub projects_succeeded: u32,
    /// Projects for which the TTL sweep returned an error.
    pub projects_failed: u32,
    /// Sum of `candidates_expired` across all successful projects.
    pub total_candidates_expired: u64,
    /// The TTL in days used for this run (`0` = sweep was a no-op).
    pub ttl_days: u32,
    /// Wall-clock duration of the sweep in milliseconds.
    pub elapsed_ms: u128,
}

// === PUBLIC API ===

/// Run one candidate-TTL sweep across every project in `registry`.
///
/// When `ttl_days == 0`, returns immediately with a zero-count report — the
/// feature is disabled. Otherwise iterates [`ProjectRegistry::project_ids`],
/// calls [`crate::workers::ProjectWorker::ttl`] for each, logs per-project
/// outcomes, and returns the aggregated report. Per-project errors are logged
/// at `warn` level and do not abort the sweep for remaining projects.
pub async fn run_once(registry: &ProjectRegistry, ttl_days: u32) -> CandidateTtlReport {
    let start = Instant::now();
    let mut report = CandidateTtlReport {
        projects_scanned: 0,
        projects_succeeded: 0,
        projects_failed: 0,
        total_candidates_expired: 0,
        ttl_days,
        elapsed_ms: 0,
    };

    if ttl_days == 0 {
        // TTL disabled — nothing to do.
        report.elapsed_ms = start.elapsed().as_millis();
        return report;
    }

    // Collect IDs first to avoid holding a reference into `registry` across
    // the `.await` points inside the loop.
    let project_ids: Vec<_> = registry.project_ids().cloned().collect();

    for project_id in project_ids {
        report.projects_scanned += 1;

        let Some(worker) = registry.get(&project_id) else {
            tracing::warn!(
                project = %project_id.as_str(),
                "candidate ttl: project disappeared from registry between id-collection and dispatch; skipping"
            );
            continue;
        };

        match worker.ttl(ttl_days).await {
            Ok(summary) => {
                report.projects_succeeded += 1;
                report.total_candidates_expired += summary.candidates_expired;
                tracing::info!(
                    project = %project_id.as_str(),
                    candidates_expired = summary.candidates_expired,
                    ttl_days = summary.ttl_days,
                    finished_at = %summary.finished_at,
                    "candidate ttl ok"
                );
            }
            Err(e) => {
                report.projects_failed += 1;
                tracing::warn!(
                    project = %project_id.as_str(),
                    error = %e,
                    "candidate ttl failed"
                );
            }
        }
    }

    report.elapsed_ms = start.elapsed().as_millis();
    report
}
