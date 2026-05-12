//! `query_events` write and read paths.
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
//!
//! # Read paths
//!
//! [`TraceFilter`] drives [`Store::fetch_traces`] (list) and
//! [`Store::fetch_trace`] (single-row lookup). Both are project-scoped.

use time::OffsetDateTime;
use ulid::Ulid;

use crate::{helpers::rfc3339, Result, Store};

// === READ TYPES ===

/// Filters for the trace list query.
///
/// All fields are optional; an empty `TraceFilter` returns up to `limit` most
/// recent traces for the project in reverse chronological order.
#[derive(Debug, Default)]
pub struct TraceFilter<'a> {
    /// Only return traces of this kind (`"search"`, `"expand"`, `"context"`).
    pub kind: Option<&'a str>,
    /// Only return traces from this caller (`"cli"` or `"mcp"`).
    pub caller: Option<&'a str>,
    /// Only return traces created at or after this RFC-3339 timestamp.
    pub since: Option<&'a str>,
    /// Maximum number of rows to return (default 10 in the engine layer).
    pub limit: u32,
}

/// One row from `query_events`, used for both list cards and full detail.
///
/// All nullable columns are `Option<String>` to reflect the schema faithfully;
/// the engine layer converts into richer presentation types.
#[derive(Debug, Clone)]
pub struct QueryEventRow {
    /// `trace_<ULID>` primary key.
    pub id: String,
    /// `"search"` | `"expand"` | `"context"`.
    pub kind: String,
    /// Requested search mode; `None` for expand / context.
    pub mode_requested: Option<String>,
    /// Resolved search mode after fallback; `None` for expand / context.
    pub mode_resolved: Option<String>,
    /// Free-text query, truncated to ≤ 1 KiB.
    pub query_text: Option<String>,
    /// JSON-serialised extra parameters.
    pub params_json: Option<String>,
    /// `"cli"` | `"mcp"`.
    pub caller: String,
    /// Embedding provider name.
    pub provider: Option<String>,
    /// Embedding model name.
    pub provider_model: Option<String>,
    /// JSON array of result memory IDs in order.
    pub result_ids_json: Option<String>,
    /// JSON array of scores parallel to `result_ids_json`.
    pub result_scores_json: Option<String>,
    /// Number of results returned.
    pub result_count: u32,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: u64,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
}

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

    /// List `query_events` rows for a project, most-recent first.
    ///
    /// Applies optional `kind`, `caller`, and `since` filters from
    /// [`TraceFilter`]. The `limit` is always applied; callers should default
    /// to 10 when unset.
    ///
    /// This is intentionally a simple cursor — no offset pagination, since
    /// the PRD does not require it for V0.3 and the default cap (10 000) keeps
    /// the result set bounded.
    pub fn fetch_traces(
        &self,
        project_id: &str,
        filter: &TraceFilter<'_>,
    ) -> Result<Vec<QueryEventRow>> {
        // Build the WHERE clause dynamically to avoid injecting NULLs into
        // SQL comparisons. SQLite handles `col = NULL` as false, not as IS NULL,
        // so we need to actually omit the clause when the filter is absent.
        let mut conditions = vec!["project_id = ?1".to_string()];
        let mut idx = 2usize;

        let mut kind_ph = None;
        let mut caller_ph = None;
        let mut since_ph = None;

        if filter.kind.is_some() {
            conditions.push(format!("kind = ?{idx}"));
            kind_ph = Some(idx);
            idx += 1;
        }
        if filter.caller.is_some() {
            conditions.push(format!("caller = ?{idx}"));
            caller_ph = Some(idx);
            idx += 1;
        }
        if filter.since.is_some() {
            conditions.push(format!("created_at >= ?{idx}"));
            since_ph = Some(idx);
            idx += 1;
        }
        let limit_ph = idx;

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT id, kind, mode_requested, mode_resolved, query_text, params_json,
                    caller, provider, provider_model,
                    result_ids_json, result_scores_json, result_count, latency_ms, created_at
             FROM query_events
             WHERE {where_clause}
             ORDER BY created_at DESC
             LIMIT ?{limit_ph}"
        );

        let mut stmt = self.connection().prepare(&sql)?;

        // Bind positional parameters in declaration order.
        // rusqlite requires we bind them by index; collect into a params vec.
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(project_id.to_string())];
        if let (Some(_), Some(k)) = (kind_ph, filter.kind) {
            params.push(Box::new(k.to_string()));
        }
        if let (Some(_), Some(c)) = (caller_ph, filter.caller) {
            params.push(Box::new(c.to_string()));
        }
        if let (Some(_), Some(s)) = (since_ph, filter.since) {
            params.push(Box::new(s.to_string()));
        }
        params.push(Box::new(filter.limit as i64));

        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                row_to_query_event,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Fetch a single `query_events` row by `trace_id`, scoped to `project_id`.
    ///
    /// Returns `Ok(None)` when the trace does not exist for the project (either
    /// genuinely absent or belonging to a different project).
    pub fn fetch_trace(&self, project_id: &str, trace_id: &str) -> Result<Option<QueryEventRow>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, kind, mode_requested, mode_resolved, query_text, params_json,
                    caller, provider, provider_model,
                    result_ids_json, result_scores_json, result_count, latency_ms, created_at
             FROM query_events
             WHERE id = ?1 AND project_id = ?2",
        )?;

        let mut rows =
            stmt.query_map(rusqlite::params![trace_id, project_id], row_to_query_event)?;

        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Fetch the traces (most-recent first) whose result set contains the
    /// given memory ID. Used by the V0.4 browser to answer "which queries
    /// returned this memory?" — the forward-link reserved in V0.3.
    ///
    /// Implementation is a `LIKE` scan over `result_ids_json`. Acceptable at
    /// the V0.3 default cap of 10 000 traces per project; if a project crosses
    /// that, V0.5+ can add a dedicated `query_event_results` join table.
    pub fn fetch_traces_for_memory(
        &self,
        project_id: &vestige_core::ProjectId,
        memory_id: &vestige_core::MemoryId,
        limit: u32,
    ) -> Result<Vec<QueryEventRow>> {
        let needle = format!("%\"{}\"%", memory_id.as_str());
        let mut stmt = self.connection().prepare(
            "SELECT id, kind, mode_requested, mode_resolved, query_text, params_json,
                    caller, provider, provider_model,
                    result_ids_json, result_scores_json, result_count, latency_ms, created_at
             FROM query_events
             WHERE project_id = ?1
               AND result_ids_json IS NOT NULL
               AND result_ids_json LIKE ?2
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![project_id.as_str(), needle, limit as i64],
                row_to_query_event,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Fetch the `id` of the most recently written `query_events` row for
    /// `project_id`, in insertion (ULID) order.
    ///
    /// Returns `Ok(None)` when the table is empty for the project. Used by
    /// the replay path to retrieve the trace ID of the row it just wrote
    /// without relying on SQLite's `last_insert_rowid` (which is unavailable
    /// through shared `&Connection` borrows).
    pub fn fetch_last_trace_id(&self, project_id: &str) -> Result<Option<String>> {
        let mut stmt = self.connection().prepare(
            "SELECT id FROM query_events
             WHERE project_id = ?1
             ORDER BY created_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![project_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }
}

// === PRIVATE HELPERS ===

fn row_to_query_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueryEventRow> {
    Ok(QueryEventRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        mode_requested: row.get(2)?,
        mode_resolved: row.get(3)?,
        query_text: row.get(4)?,
        params_json: row.get(5)?,
        caller: row.get(6)?,
        provider: row.get(7)?,
        provider_model: row.get(8)?,
        result_ids_json: row.get(9)?,
        result_scores_json: row.get(10)?,
        result_count: row.get::<_, i64>(11)? as u32,
        latency_ms: row.get::<_, i64>(12)? as u64,
        created_at: row.get(13)?,
    })
}
