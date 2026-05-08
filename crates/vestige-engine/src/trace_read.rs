//! Engine read API for query traces (`vestige trace list / show`).
//!
//! Complements [`crate::trace`] (the write path). All reads are project-scoped
//! and surface [`TraceCard`] (compact list) or [`TraceDetail`] (full single-row
//! view). Both shapes serialise to the JSON documented in PRD §13.3.
//!
//! # Design
//!
//! Neither the CLI nor the MCP layer should touch `vestige-store` directly for
//! reads — all query logic lives here so both surfaces share one implementation.
//! The CLI passes a [`ListFilters`] built from flag parsing; the MCP tool passes
//! the same struct built from its JSON input.

use serde::{Deserialize, Serialize};
use vestige_core::{ProjectId, TraceId};
use vestige_store::{QueryEventRow, Store, TraceFilter};

use crate::error::{EngineError, Result};

// === DEFAULT CONSTANTS ===

/// Default number of traces to return when `--limit` is not specified.
pub const DEFAULT_TRACE_LIMIT: u32 = 10;

// === PUBLIC TYPES ===

/// Compact trace representation for the list view.
///
/// Matches the `traces[]` element in PRD §13.3 (MCP list output) and the
/// `vestige trace` text row format in §7.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceCard {
    /// `trace_<ULID>` primary key.
    pub trace_id: String,
    /// `"search"` | `"expand"` | `"context"`.
    pub kind: String,
    /// Resolved search mode (`"lexical"` / `"semantic"` / `"hybrid"`); `None`
    /// for expand and context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Query text (up to 30 chars in text display; full text in JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Number of results returned.
    pub result_count: u32,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: u64,
    /// `"cli"` | `"mcp"`.
    pub caller: String,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
}

/// Full trace detail for `vestige trace <trace_id>`.
///
/// Matches the `vestige trace <id>` text output in PRD §7.3 and the `show`
/// action in the MCP `vestige_trace` tool (§10.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceDetail {
    /// `trace_<ULID>` primary key.
    pub trace_id: String,
    /// `"search"` | `"expand"` | `"context"`.
    pub kind: String,
    /// Requested mode (search only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_requested: Option<String>,
    /// Resolved mode after fallback (search only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_resolved: Option<String>,
    /// Full query text (up to 1 KiB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// JSON-serialised extra parameters (limit, type_filter, depth, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// `"cli"` | `"mcp"`.
    pub caller: String,
    /// Embedding provider name (search with semantic/hybrid only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Embedding model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
    /// Ordered list of result memory IDs.
    pub result_ids: Vec<String>,
    /// Scores parallel to `result_ids`; empty for expand / context.
    pub result_scores: Vec<f64>,
    /// Number of results (mirrors `result_ids.len()` for fast display).
    pub result_count: u32,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: u64,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
}

/// Filters for `vestige trace` (list mode).
#[derive(Debug, Default)]
pub struct ListFilters<'a> {
    /// Only return traces of this kind.
    pub kind: Option<&'a str>,
    /// Only return traces from this caller.
    pub caller: Option<&'a str>,
    /// Only return traces created at or after this ISO-8601 date/datetime.
    pub since: Option<&'a str>,
    /// Maximum rows to return (defaults to [`DEFAULT_TRACE_LIMIT`]).
    pub limit: u32,
}

// === PUBLIC API ===

/// List recent traces for the current project.
///
/// Delegates to [`Store::fetch_traces`] after normalising `since` to an
/// RFC-3339 string. Returns at most `filters.limit` rows in reverse
/// chronological order.
///
/// # Errors
///
/// - [`EngineError::Validation`] — `kind` or `caller` filter has an
///   unrecognised value; `since` cannot be parsed.
/// - [`EngineError::Store`] — SQLite read failure.
pub fn list_traces<'a>(
    store: &Store,
    project_id: &ProjectId,
    filters: &ListFilters<'a>,
) -> Result<Vec<TraceCard>> {
    validate_kind_filter(filters.kind)?;
    validate_caller_filter(filters.caller)?;
    let since_str = normalise_since(filters.since)?;

    let limit = if filters.limit == 0 {
        DEFAULT_TRACE_LIMIT
    } else {
        filters.limit
    };

    let store_filter = TraceFilter {
        kind: filters.kind,
        caller: filters.caller,
        since: since_str.as_deref(),
        limit,
    };

    let rows = store.fetch_traces(project_id.as_str(), &store_filter)?;
    Ok(rows.into_iter().map(row_to_card).collect())
}

/// Fetch a single trace by ID, scoped to the current project.
///
/// # Errors
///
/// - [`EngineError::Validation`] — `trace_id` does not parse as a valid
///   `trace_<ULID>` string.
/// - [`EngineError::Store`] — SQLite read failure.
/// - [`EngineError::TraceNotFound`] — no trace with this ID in the current
///   project.
pub fn get_trace(store: &Store, project_id: &ProjectId, trace_id: &TraceId) -> Result<TraceDetail> {
    let row = store.fetch_trace(project_id.as_str(), trace_id.as_str())?;
    match row {
        Some(r) => Ok(row_to_detail(r)?),
        None => Err(EngineError::TraceNotFound {
            id: trace_id.as_str().to_string(),
        }),
    }
}

// === PRIVATE HELPERS ===

fn row_to_card(row: QueryEventRow) -> TraceCard {
    TraceCard {
        trace_id: row.id,
        kind: row.kind,
        mode: row.mode_resolved,
        query: row.query_text,
        result_count: row.result_count,
        latency_ms: row.latency_ms,
        caller: row.caller,
        created_at: row.created_at,
    }
}

fn row_to_detail(row: QueryEventRow) -> Result<TraceDetail> {
    let result_ids: Vec<String> = match &row.result_ids_json {
        Some(json) => serde_json::from_str(json).unwrap_or_default(),
        None => vec![],
    };

    let result_scores: Vec<f64> = match &row.result_scores_json {
        Some(json) => serde_json::from_str(json).unwrap_or_default(),
        None => vec![],
    };

    let params: Option<serde_json::Value> = match &row.params_json {
        Some(json) => serde_json::from_str(json).ok(),
        None => None,
    };

    Ok(TraceDetail {
        trace_id: row.id,
        kind: row.kind,
        mode_requested: row.mode_requested,
        mode_resolved: row.mode_resolved,
        query: row.query_text,
        params,
        caller: row.caller,
        provider: row.provider,
        provider_model: row.provider_model,
        result_ids,
        result_scores,
        result_count: row.result_count,
        latency_ms: row.latency_ms,
        created_at: row.created_at,
    })
}

/// Validate that `kind`, if present, is one of the three known values.
fn validate_kind_filter(kind: Option<&str>) -> Result<()> {
    match kind {
        None | Some("search") | Some("expand") | Some("context") => Ok(()),
        Some(unknown) => Err(EngineError::Validation {
            message: format!(
                "unknown trace kind `{unknown}` — expected one of: search, expand, context"
            ),
        }),
    }
}

/// Validate that `caller`, if present, is `"cli"` or `"mcp"`.
fn validate_caller_filter(caller: Option<&str>) -> Result<()> {
    match caller {
        None | Some("cli") | Some("mcp") => Ok(()),
        Some(unknown) => Err(EngineError::Validation {
            message: format!("unknown caller `{unknown}` — expected one of: cli, mcp"),
        }),
    }
}

/// Normalise a `--since` input to a RFC-3339 string (or `None`).
///
/// Accepts ISO-8601 dates (`2026-05-08`) and RFC-3339 datetimes
/// (`2026-05-08T14:00:00Z`, `2026-05-08T14:00:00+01:00`).
/// Returns an error on any other input.
fn normalise_since(since: Option<&str>) -> Result<Option<String>> {
    let s = match since {
        None => return Ok(None),
        Some(s) => s,
    };

    // Try full RFC-3339 first.
    if time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_ok() {
        return Ok(Some(s.to_string()));
    }

    // Try ISO-8601 date only (`YYYY-MM-DD`) and expand to midnight UTC.
    if let Ok(date) = time::Date::parse(
        s,
        &time::format_description::parse("[year]-[month]-[day]").unwrap(),
    ) {
        let dt = date.with_time(time::Time::MIDNIGHT).assume_utc();
        let formatted = dt
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| EngineError::Validation {
                message: format!("failed to format `--since` date: {e}"),
            })?;
        return Ok(Some(formatted));
    }

    Err(EngineError::Validation {
        message: format!(
            "invalid `--since` value `{s}` — expected ISO-8601 date (2026-05-08) or RFC-3339 datetime"
        ),
    })
}
