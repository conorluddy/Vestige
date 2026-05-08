//! `vestige_trace` tool — inspect and replay query traces (PRD §10.3).
//!
//! A single tool with three actions dispatched by the `action` field:
//!
//! - `list`   — paginated list of recent traces, optionally filtered by kind/caller/since.
//! - `show`   — full detail for a single trace ID.
//! - `replay` — re-run a stored trace against the current store; writes one new
//!   `query_events` row tagged `caller=mcp` and `params_json.replay_of`.
//!
//! All errors use the structured `{code, message, retryable}` shape expected by agents.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_config::embeddings_config_for;
use vestige_core::TraceId;
use vestige_embed::build_provider;
use vestige_engine::error::EngineError;
use vestige_engine::trace_read::DEFAULT_TRACE_LIMIT;
use vestige_engine::Caller;
use vestige_engine::{
    get_trace, list_traces, replay_trace, ListFilters, ReplayResult, TraceCard, TraceDetail,
};

use crate::server::{err, ok_json, Inner, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TraceParams {
    /// Action to perform: `list` | `show` | `replay`.
    pub action: String,

    /// Trace ID — required for `show` and `replay`; ignored for `list`.
    #[serde(default)]
    pub trace_id: Option<String>,

    /// Maximum traces to return (list only). Defaults to 10.
    #[serde(default = "default_trace_limit")]
    pub limit: u32,

    /// Filter by kind: `search` | `expand` | `context` (list only).
    #[serde(default)]
    pub kind: Option<String>,

    /// Filter by caller: `cli` | `mcp` (list only).
    #[serde(default)]
    pub caller: Option<String>,

    /// Return only traces created at or after this ISO-8601 date or RFC-3339 datetime (list only).
    #[serde(default)]
    pub since: Option<String>,
}

fn default_trace_limit() -> u32 {
    DEFAULT_TRACE_LIMIT
}

// === OUTPUT SHAPES ===

#[derive(Debug, Serialize)]
struct ListResponse {
    traces: Vec<TraceCard>,
}

#[derive(Debug, Serialize)]
struct ShowResponse {
    #[serde(flatten)]
    detail: TraceDetail,
}

/// MCP replay response — wraps `ReplayResult` and adds `corpus_drift` per PRD §10.3.
///
/// `corpus_drift` = absolute change in `result_count` between original and current
/// (`corpus_size` from the engine minus the original `result_count`). Positive means
/// the corpus has grown; negative means it has shrunk.
#[derive(Debug, Serialize)]
struct ReplayResponse {
    #[serde(flatten)]
    result: ReplayResult,
    /// Difference in result-set cardinality: `current.result_ids.len()` - original count.
    corpus_drift: i64,
}

// === TOOL ROUTER ===

#[tool_router(router = trace_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(description = "Inspect and replay query traces. \
                       action=list returns recent traces (filterable by kind/caller/since, \
                       default limit 10). \
                       action=show returns full detail for a given trace_id. \
                       action=replay re-runs a stored trace against the current store, \
                       diffs the results, and writes a new query_events row tagged \
                       caller=mcp with replay_of in params_json.")]
    pub async fn vestige_trace(
        &self,
        Parameters(p): Parameters<TraceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        match p.action.as_str() {
            "list" => handle_list(&inner, &p),
            "show" => handle_show(&inner, &p),
            "replay" => handle_replay(&inner, &p),
            other => Err(err(
                "INVALID_ACTION",
                format!("unknown action `{other}` — expected one of: list, show, replay"),
                false,
            )),
        }
    }
}

// === ACTION HANDLERS ===

fn handle_list(inner: &Inner, p: &TraceParams) -> Result<CallToolResult, ErrorData> {
    let filters = ListFilters {
        kind: p.kind.as_deref(),
        caller: p.caller.as_deref(),
        since: p.since.as_deref(),
        limit: p.limit,
    };

    let traces =
        list_traces(&inner.store, &inner.project_id, &filters).map_err(map_engine_error)?;

    ok_json(&ListResponse { traces })
}

fn handle_show(inner: &Inner, p: &TraceParams) -> Result<CallToolResult, ErrorData> {
    let raw_id = p.trace_id.as_deref().ok_or_else(|| {
        err(
            "MISSING_PARAM",
            "trace_id is required for action=show",
            false,
        )
    })?;

    let trace_id = raw_id
        .parse::<TraceId>()
        .map_err(|e| err("INVALID_TRACE_ID", e.to_string(), false))?;

    let detail = get_trace(&inner.store, &inner.project_id, &trace_id).map_err(map_engine_error)?;

    ok_json(&ShowResponse { detail })
}

fn handle_replay(inner: &Inner, p: &TraceParams) -> Result<CallToolResult, ErrorData> {
    let raw_id = p.trace_id.as_deref().ok_or_else(|| {
        err(
            "MISSING_PARAM",
            "trace_id is required for action=replay",
            false,
        )
    })?;

    let trace_id = raw_id
        .parse::<TraceId>()
        .map_err(|e| err("INVALID_TRACE_ID", e.to_string(), false))?;

    // Build the embedding provider from project config (defaults to `fake`).
    let cfg = embeddings_config_for(inner.config.embeddings.as_ref());
    let provider =
        build_provider(&cfg).map_err(|e| err("PROVIDER_INIT_FAILED", e.to_string(), false))?;

    let result = replay_trace(
        &inner.store,
        Some(provider.as_ref()),
        &inner.project_id,
        &trace_id,
        Caller::Mcp,
    )
    .map_err(map_engine_error)?;

    // Compute corpus_drift: current result count minus original result count.
    let corpus_drift =
        result.current.result_ids.len() as i64 - result.original.result_ids.len() as i64;

    ok_json(&ReplayResponse {
        result,
        corpus_drift,
    })
}

// === PRIVATE HELPERS ===

fn map_engine_error(e: EngineError) -> ErrorData {
    match e {
        EngineError::TraceNotFound { ref id } => {
            err("TRACE_NOT_FOUND", format!("trace not found: `{id}`"), false)
        }
        EngineError::Validation { ref message } => err("VALIDATION", message.clone(), false),
        EngineError::Store(_) => err("STORE_FAILED", e.to_string(), true),
        EngineError::Embed(_) => err("EMBED_FAILED", e.to_string(), false),
        EngineError::EmbeddingsUnavailable(_) => {
            err("EMBEDDINGS_UNAVAILABLE", e.to_string(), false)
        }
        EngineError::OutOfScope => err("OUT_OF_SCOPE", e.to_string(), false),
        EngineError::Core(_) => err("CORE_ERROR", e.to_string(), false),
        EngineError::CandidateNotFound { ref id } => err(
            "CANDIDATE_NOT_FOUND",
            format!("candidate not found: `{id}`"),
            false,
        ),
        EngineError::CandidateNotPending { ref status } => err(
            "CANDIDATE_NOT_PENDING",
            format!("candidate is not pending (status = {status})"),
            false,
        ),
    }
}
