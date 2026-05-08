//! Engine tracing hook — single write site for all recall `query_events` rows.
//!
//! Every recall path (`search_*`, `expand`, `get_project_context`) calls
//! [`write_trace_configured`] (config-driven) or [`write_trace`] (legacy
//! default-cap path) after it resolves its result. Mutation paths (record,
//! forget, restore, approve, reject) never call this module.
//!
//! # Placement discipline
//!
//! The engine is the **only** trace write site. Neither `vestige-cli` nor
//! `vestige-mcp` write `query_events` rows directly — they pass a [`Caller`]
//! variant and the engine records the row. This ensures the cap, eviction, and
//! field population logic live in exactly one place and both surfaces stay in
//! sync automatically.
//!
//! # Failure isolation (PRD §10.5)
//!
//! A trace-write failure must never propagate to the caller as a recall error.
//! [`write_trace`] logs at `warn` level and returns `Ok(())` on store failure
//! so the hot recall path is not interrupted.
//!
//! # FIFO eviction
//!
//! Rows per project are kept up to the configured `max_per_project` (default
//! [`TRACE_CAP`]). After every insert the store checks the count and deletes
//! the oldest `(count - cap)` rows in one SQL statement.
//! In tests the cap can be overridden via [`write_trace_with_cap`].
//!
//! # Config-driven behaviour (V0.3 M7)
//!
//! [`write_trace_configured`] accepts a [`TracesConfig`] and applies:
//! - `enabled = false` → skip the write entirely.
//! - `trace_caller_cli / trace_caller_mcp` → skip writes for that surface.
//! - `max_per_project` → FIFO cap instead of [`TRACE_CAP`].
//! - `truncate_query_text_bytes` → `query_text` is byte-truncated at the
//!   nearest UTF-8 codepoint boundary before writing.
//!
//! The legacy [`write_trace`] and [`write_trace_with_cap`] keep their
//! existing signatures unchanged for compatibility with tests and any
//! call sites that pre-date config support.

use std::time::Instant;

use serde::Serialize;
use tracing::warn;
use vestige_config::TracesConfig;
use vestige_core::{MemoryId, ProjectId, SearchMode};
use vestige_store::{NewQueryEvent, Store};

// === CONSTANTS ===

/// Hard-coded per-project `query_events` cap. Promoted to config in M7.
pub const TRACE_CAP: usize = 10_000;

// === TYPES ===

/// The surface that initiated a recall call.
///
/// Stored verbatim in `query_events.caller` as `"cli"` or `"mcp"`.
/// Both `vestige-cli` and `vestige-mcp` are required to pass the correct
/// variant; the engine cannot infer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Cli,
    Mcp,
}

impl Caller {
    /// String stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Caller::Cli => "cli",
            Caller::Mcp => "mcp",
        }
    }
}

/// Kind of recall operation.
///
/// Maps 1:1 to the `kind` column values in `query_events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceKind {
    Search,
    Expand,
    Context,
}

impl TraceKind {
    fn as_str(self) -> &'static str {
        match self {
            TraceKind::Search => "search",
            TraceKind::Expand => "expand",
            TraceKind::Context => "context",
        }
    }
}

/// Everything the engine needs to build a `query_events` row.
///
/// Constructed by each recall function and passed to [`write_trace`].
/// Fields map directly to the `query_events` schema (migration 0005).
pub struct TracePayload<'a> {
    /// Project the recall ran against.
    pub project_id: &'a ProjectId,
    /// Kind of recall.
    pub kind: TraceKind,
    /// For `Search` calls: the requested mode.
    pub mode_requested: Option<SearchMode>,
    /// For `Search` calls: the effective mode after fallback.
    pub mode_resolved: Option<SearchMode>,
    /// Free-text query string (null for `Context`).
    pub query_text: Option<&'a str>,
    /// JSON-serialised extra parameters. The engine builds this; callers do
    /// not need to pre-serialise.
    pub params_json: Option<String>,
    /// Surface that originated the call.
    pub caller: Caller,
    /// Embedding provider name; null for lexical / non-search.
    pub provider: Option<&'a str>,
    /// Embedding model name; null when provider is null.
    pub provider_model: Option<&'a str>,
    /// Ordered result memory IDs (null for expand / context).
    pub result_ids: Option<&'a [MemoryId]>,
    /// Scores parallel to `result_ids` (null for expand / context).
    pub result_scores: Option<&'a [f64]>,
    /// Elapsed time for the recall operation.
    pub latency: std::time::Duration,
}

// === PUBLIC API ===

/// Record one `query_events` row with the default [`TRACE_CAP`] and evict on overflow.
///
/// On store failure the error is logged at `warn` and the function returns
/// `Ok(())` — trace-write failure must never abort a successful recall
/// (PRD §10.5).
pub fn write_trace(store: &Store, payload: &TracePayload<'_>) {
    write_trace_with_cap(store, payload, TRACE_CAP);
}

/// Like [`write_trace`] but accepts an explicit `cap` — used in tests to
/// exercise eviction without inserting 10 000 rows.
pub fn write_trace_with_cap(store: &Store, payload: &TracePayload<'_>, cap: usize) {
    let result_ids_json = payload.result_ids.map(|ids| {
        serde_json::to_string(&ids.iter().map(|id| id.as_str()).collect::<Vec<_>>())
            .unwrap_or_default()
    });

    let result_scores_json = payload
        .result_scores
        .map(|scores| serde_json::to_string(scores).unwrap_or_default());

    let result_count = payload.result_ids.map(|ids| ids.len() as u32).unwrap_or(0);
    let latency_ms = payload.latency.as_millis() as u64;

    let event = NewQueryEvent {
        kind: payload.kind.as_str(),
        project_id: payload.project_id.as_str(),
        mode_requested: payload.mode_requested.map(search_mode_str),
        mode_resolved: payload.mode_resolved.map(search_mode_str),
        query_text: payload.query_text,
        params_json: payload.params_json.as_deref(),
        caller: payload.caller.as_str(),
        provider: payload.provider,
        provider_model: payload.provider_model,
        result_ids_json: result_ids_json.as_deref(),
        result_scores_json: result_scores_json.as_deref(),
        result_count,
        latency_ms,
    };

    if let Err(e) = store.record_query_event(&event, cap) {
        warn!("trace write failed (non-fatal): {e}");
    }
}

/// Config-driven trace write — the preferred call site for all engine recall
/// paths that have access to a resolved [`TracesConfig`].
///
/// Applies the following checks before writing:
/// 1. `enabled = false` → skip (no-op).
/// 2. `trace_caller_cli / trace_caller_mcp = false` → skip for that surface.
/// 3. `query_text` is truncated to `truncate_query_text_bytes` at the nearest
///    UTF-8 codepoint boundary (bytes, not chars — PRD bytes-not-chars rule).
/// 4. `max_per_project` drives FIFO eviction instead of [`TRACE_CAP`].
///
/// On store failure the error is logged at `warn` and the function returns
/// without propagating — PRD §10.5 failure-isolation rule.
pub fn write_trace_configured(store: &Store, payload: &TracePayload<'_>, cfg: &TracesConfig) {
    // 1. Master switch.
    if !cfg.enabled {
        return;
    }

    // 2. Per-surface toggle.
    match payload.caller {
        Caller::Cli if !cfg.trace_caller_cli => return,
        Caller::Mcp if !cfg.trace_caller_mcp => return,
        _ => {}
    }

    // 3. Truncate query_text at a UTF-8 codepoint boundary.
    let truncated_query: Option<String>;
    let query_text = match payload.query_text {
        Some(q) if q.len() > cfg.truncate_query_text_bytes => {
            truncated_query = Some(truncate_at_char_boundary(q, cfg.truncate_query_text_bytes));
            truncated_query.as_deref()
        }
        other => other,
    };

    // 4. Build a payload with the (possibly truncated) query_text and delegate.
    let effective = TracePayload {
        project_id: payload.project_id,
        kind: payload.kind,
        mode_requested: payload.mode_requested,
        mode_resolved: payload.mode_resolved,
        query_text,
        params_json: payload.params_json.clone(),
        caller: payload.caller,
        provider: payload.provider,
        provider_model: payload.provider_model,
        result_ids: payload.result_ids,
        result_scores: payload.result_scores,
        latency: payload.latency,
    };
    write_trace_with_cap(store, &effective, cfg.max_per_project);
}

// === HELPERS ===

/// Truncate `s` to at most `max_bytes` bytes, stepping back to the nearest
/// UTF-8 codepoint boundary so the result is always valid UTF-8.
///
/// PRD rule: truncation is byte-based, not char-based — consistent with the
/// 2 KiB source-snippet cap in `vestige-core`.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Walk backwards from max_bytes to find the nearest codepoint boundary.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn search_mode_str(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Lexical => "lexical",
        SearchMode::Semantic => "semantic",
        SearchMode::Hybrid => "hybrid",
    }
}

/// Build the `params_json` string for a search call.
pub fn search_params_json(limit: u32, type_filter: Option<&str>) -> String {
    #[derive(Serialize)]
    struct SearchParams<'a> {
        limit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        type_filter: Option<&'a str>,
    }
    serde_json::to_string(&SearchParams { limit, type_filter }).unwrap_or_default()
}

/// Build the `params_json` string for an expand call.
pub fn expand_params_json(depth: &str) -> String {
    #[derive(Serialize)]
    struct ExpandParams<'a> {
        depth: &'a str,
    }
    serde_json::to_string(&ExpandParams { depth }).unwrap_or_default()
}

/// Build the `params_json` string for a context call.
pub fn context_params_json(budget_tokens: usize, per_section: u32) -> String {
    #[derive(Serialize)]
    struct ContextParams {
        budget_tokens: usize,
        per_section: u32,
    }
    serde_json::to_string(&ContextParams {
        budget_tokens,
        per_section,
    })
    .unwrap_or_default()
}

/// Convenience: capture start time. Use with [`elapsed_since`].
pub fn start_timer() -> Instant {
    Instant::now()
}

/// Elapsed duration since `start`.
pub fn elapsed_since(start: Instant) -> std::time::Duration {
    start.elapsed()
}
