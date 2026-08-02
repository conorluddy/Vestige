//! Session-log scan job: ask each project worker to mine its local transcripts into
//! candidates (V0.5.4).
//!
//! Thin orchestration shim, mirroring [`embed_sweep`](super::embed_sweep). The real work —
//! discovery, redaction, LLM extraction, and candidate proposal — happens inside each worker
//! thread via [`ProjectWorker::scan_sessions`], which delegates to
//! [`vestige_engine::scan_and_propose`]. This job fans out to every registered project,
//! collects results, and logs outcomes.
//!
//! Projects whose `[extraction]` provider is unavailable are counted as `skipped` (no-op),
//! never failed — the daemon never dumps raw turns as candidates.

use std::time::Instant;

use crate::registry::ProjectRegistry;

// === TYPES ===

/// Aggregate outcome of one session-log scan across all registered projects.
#[derive(Debug, Clone)]
pub struct SessionLogScanReport {
    /// Total number of projects the scan attempted.
    pub projects_scanned: u32,
    /// Projects whose scan completed (provider available).
    pub projects_succeeded: u32,
    /// Projects whose scan errored.
    pub projects_failed: u32,
    /// Projects skipped because no extraction provider was available.
    pub projects_skipped: u32,
    /// Sum of `candidates_proposed` across all successful projects.
    pub total_candidates_proposed: u64,
    /// Wall-clock duration of the scan in milliseconds.
    pub elapsed_ms: u128,
}

// === PUBLIC API ===

/// Run one session-log scan across every project in `registry`.
///
/// Errors in individual projects are logged at `warn` level and counted — they never abort
/// the scan for the remaining projects.
pub async fn run_once(registry: &ProjectRegistry) -> SessionLogScanReport {
    let start = Instant::now();
    let mut report = SessionLogScanReport {
        projects_scanned: 0,
        projects_succeeded: 0,
        projects_failed: 0,
        projects_skipped: 0,
        total_candidates_proposed: 0,
        elapsed_ms: 0,
    };

    // Collect IDs first to avoid holding a reference into `registry` across `.await`.
    let project_ids: Vec<_> = registry.project_ids().cloned().collect();

    for project_id in project_ids {
        report.projects_scanned += 1;

        let Some(worker) = registry.get(&project_id) else {
            tracing::warn!(
                project = %project_id.as_str(),
                "session scan: project disappeared from registry between id-collection and dispatch; skipping"
            );
            continue;
        };

        match worker.scan_sessions().await {
            Ok(summary) if summary.skipped => {
                report.projects_skipped += 1;
                tracing::debug!(
                    project = %project_id.as_str(),
                    "session scan skipped: no extraction provider configured"
                );
            }
            Ok(summary) => {
                report.projects_succeeded += 1;
                report.total_candidates_proposed += summary.candidates_proposed;
                tracing::info!(
                    project = %project_id.as_str(),
                    sessions_scanned = summary.sessions_scanned,
                    candidates_proposed = summary.candidates_proposed,
                    finished_at = %summary.finished_at,
                    "session scan ok"
                );
            }
            Err(e) => {
                report.projects_failed += 1;
                tracing::warn!(
                    project = %project_id.as_str(),
                    error = %e,
                    "session scan failed"
                );
            }
        }
    }

    report.elapsed_ms = start.elapsed().as_millis();
    report
}
