//! `query_events` write path — record one trace row and evict oldest when over cap.
//!
//! All recall paths (search / expand / context) write exactly one row per call.
//! Mutations (record, forget, restore, approve, reject) write zero rows — this
//! module is never called from those paths.
//!
//! # FIFO eviction
//!
//! After every successful insert we check the per-project row count. When it
//! exceeds `cap` we delete the oldest `(count - cap)` rows in a single
//! DELETE statement. The cap is enforced by the engine layer, which passes it
//! in; keeping it here avoids a second trip to the DB for the count query.

use time::OffsetDateTime;
use ulid::Ulid;

use crate::{helpers::rfc3339, Result, Store};

/// Everything needed to insert one `query_events` row.
///
/// All nullable columns use `Option<&'a str>` to avoid heap allocation for
/// the common cases where provider / scores / etc. are absent.
pub struct NewQueryEvent<'a> {
    /// One of `"search"`, `"expand"`, or `"context"`.
    pub kind: &'a str,
    /// The project this trace belongs to.
    pub project_id: &'a str,
    /// Requested search mode (`"lexical"` / `"semantic"` / `"hybrid"`); null
    /// for expand and context.
    pub mode_requested: Option<&'a str>,
    /// Resolved search mode after fallback; null for expand and context.
    pub mode_resolved: Option<&'a str>,
    /// Raw query string (≤ 1 KiB enforced at M7; stored verbatim for now).
    pub query_text: Option<&'a str>,
    /// JSON-serialised extra parameters (limit, type_filter, depth, …).
    pub params_json: Option<&'a str>,
    /// `"cli"` or `"mcp"`.
    pub caller: &'a str,
    /// Embedding provider name; null for lexical / non-search.
    pub provider: Option<&'a str>,
    /// Embedding model name; null when provider is null.
    pub provider_model: Option<&'a str>,
    /// JSON array of `mem_<ULID>` strings in result order; null for
    /// expand / context.
    pub result_ids_json: Option<&'a str>,
    /// JSON array of scores parallel to `result_ids_json`; null for
    /// expand / context.
    pub result_scores_json: Option<&'a str>,
    /// Number of results returned.
    pub result_count: u32,
    /// Wall-clock latency of the recall operation in milliseconds.
    pub latency_ms: u64,
}

impl Store {
    /// Insert one `query_events` row and evict the oldest rows if the
    /// per-project count exceeds `cap`.
    ///
    /// # Eviction
    ///
    /// Eviction runs **after** the insert so the newest row is never the one
    /// deleted. A single `DELETE … WHERE id IN (SELECT … LIMIT N)` keeps the
    /// operation O(1) in SQL round-trips regardless of how many rows need
    /// removing (normally 0 or 1).
    ///
    /// # Failure isolation
    ///
    /// This method returns `Ok(())` even when called from a path where the
    /// query already succeeded. The caller (engine) is responsible for
    /// logging the error and continuing — per PRD §10.5, a trace-write
    /// failure must never cause a recall operation to fail.
    ///
    /// In practice the engine wraps this in `if let Err(e) = store.record_query_event(…)`.
    pub fn record_query_event(&self, event: &NewQueryEvent<'_>, cap: usize) -> Result<()> {
        let id = format!("trace_{}", Ulid::new());
        let now_str = rfc3339(OffsetDateTime::now_utc())?;

        self.connection().execute(
            "INSERT INTO query_events (
                 id, project_id, kind, mode_requested, mode_resolved,
                 query_text, params_json, caller,
                 provider, provider_model,
                 result_ids_json, result_scores_json, result_count,
                 latency_ms, created_at
             ) VALUES (
                 ?1,  ?2,  ?3,  ?4,  ?5,
                 ?6,  ?7,  ?8,
                 ?9,  ?10,
                 ?11, ?12, ?13,
                 ?14, ?15
             )",
            rusqlite::params![
                id,
                event.project_id,
                event.kind,
                event.mode_requested,
                event.mode_resolved,
                event.query_text,
                event.params_json,
                event.caller,
                event.provider,
                event.provider_model,
                event.result_ids_json,
                event.result_scores_json,
                event.result_count,
                event.latency_ms as i64,
                now_str,
            ],
        )?;

        self.evict_query_events(event.project_id, cap)?;
        Ok(())
    }

    /// Count `query_events` rows for `project_id` and delete the oldest
    /// `(count - cap)` rows when over the limit.
    ///
    /// Uses a single subquery-DELETE so the work is one SQL statement.
    /// Called only from [`record_query_event`]; kept private.
    fn evict_query_events(&self, project_id: &str, cap: usize) -> Result<()> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM query_events WHERE project_id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )?;

        let excess = count as usize;
        if excess <= cap {
            return Ok(());
        }

        let to_delete = excess - cap;
        self.connection().execute(
            "DELETE FROM query_events
             WHERE id IN (
                 SELECT id FROM query_events
                 WHERE project_id = ?1
                 ORDER BY created_at ASC
                 LIMIT ?2
             )",
            rusqlite::params![project_id, to_delete as i64],
        )?;
        Ok(())
    }

    /// Count `query_events` rows for a project.
    ///
    /// Primarily used in integration tests to verify trace-write behaviour and
    /// FIFO eviction. Also useful for CLI status commands in future milestones.
    pub fn query_event_count(&self, project_id: &str) -> Result<usize> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM query_events WHERE project_id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )?;
        Ok(count as usize)
    }
}
