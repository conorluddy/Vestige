//! Engine wrappers for expand and context-pack recall paths.
//!
//! Both `vestige_expand` (MCP) and `vestige show` (CLI) expand a single
//! memory to a chosen representation depth. Both `vestige_get_project_context`
//! (MCP) and `vestige context` (CLI) build a budget-bounded pack.
//!
//! These engine functions are the **single trace write site** for both paths.
//! They call into `vestige-core` and `vestige-store` for the actual work and
//! write one `query_events` row via [`crate::trace::write_trace`] after the
//! result is resolved.
//!
//! # PRD §10.5
//!
//! A trace-write failure must never abort a successful recall. Trace failures
//! are logged at `warn` and swallowed; the function returns the recall result
//! regardless.

use vestige_core::{
    build_pack, project_detail, ContextOptions, ContextSources, FetchedMemory, ListFilter,
    MemoryId, MemoryType, ProjectId, RepresentationDepth,
};
use vestige_store::Store;

use crate::error::{EngineError, Result};
use crate::trace::{
    context_params_json, elapsed_since, expand_params_json, start_timer, write_trace, Caller,
    TraceKind, TracePayload,
};

// === TYPES ===

/// Return value from [`expand_memory`].
pub struct ExpandOutcome {
    /// The fetched memory with all representations.
    pub fetched: FetchedMemory,
    /// The text content at the requested depth (empty string when depth absent).
    pub content: String,
}

/// Return value from [`get_project_context`].
pub struct ContextOutcome {
    /// The assembled text pack.
    pub pack: vestige_core::ContextPack,
}

// === PUBLIC API ===

/// Fetch a single memory and expand it to `depth`.
///
/// Returns `EngineError::Store` when the memory is not found or the SQLite
/// query fails. Scope-check (project boundary) is the caller's responsibility;
/// this function only validates that `id` resolves to a row.
///
/// One `query_events` row with `kind = "expand"` is written after the result
/// is resolved. Trace failure is logged and swallowed.
pub fn expand_memory(
    store: &Store,
    project_id: &ProjectId,
    id: &MemoryId,
    depth: RepresentationDepth,
    caller: Caller,
) -> Result<ExpandOutcome> {
    let t0 = start_timer();

    let fetched = store.get_memory(id)?.ok_or_else(|| {
        EngineError::Store(vestige_store::StoreError::Corruption(format!(
            "memory not found: {id}"
        )))
    })?;

    let detail = project_detail(&fetched);
    let content = detail
        .representations
        .iter()
        .find(|(d, _)| *d == depth)
        .map(|(_, c)| c.clone())
        .unwrap_or_default();

    let latency = elapsed_since(t0);
    let params_json = expand_params_json(depth.as_str());

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Expand,
            mode_requested: None,
            mode_resolved: None,
            query_text: Some(id.as_str()),
            params_json: Some(params_json),
            caller,
            provider: None,
            provider_model: None,
            result_ids: None,
            result_scores: None,
            latency,
        },
    );

    Ok(ExpandOutcome { fetched, content })
}

/// Build a budget-bounded context pack for `project_id`.
///
/// Lists memories by type (summary, decisions, open questions, recent) and
/// assembles them into a [`vestige_core::ContextPack`]. One `query_events`
/// row with `kind = "context"` is written after the pack is built. Trace
/// failure is logged and swallowed.
pub fn get_project_context(
    store: &Store,
    project_id: &ProjectId,
    project_name: &str,
    per_section: u32,
    budget_tokens: usize,
    caller: Caller,
) -> Result<ContextOutcome> {
    let t0 = start_timer();

    let summary = store
        .list_memories(
            project_id,
            &ListFilter {
                include_deleted: false,
                r#type: Some(MemoryType::ProjectSummary),
                limit: Some(1),
            },
        )?
        .into_iter()
        .next();

    let decisions = store.list_memories(
        project_id,
        &ListFilter {
            include_deleted: false,
            r#type: Some(MemoryType::Decision),
            limit: Some(per_section),
        },
    )?;

    let open_questions = store.list_memories(
        project_id,
        &ListFilter {
            include_deleted: false,
            r#type: Some(MemoryType::OpenQuestion),
            limit: Some(per_section),
        },
    )?;

    let recent = store.list_memories(
        project_id,
        &ListFilter {
            include_deleted: false,
            r#type: None,
            limit: Some(per_section),
        },
    )?;

    let pack = build_pack(
        ContextSources {
            project_name: project_name.to_string(),
            summary,
            decisions,
            open_questions,
            recent,
        },
        ContextOptions { budget_tokens },
    );

    let latency = elapsed_since(t0);
    let params_json = context_params_json(budget_tokens, per_section);

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Context,
            mode_requested: None,
            mode_resolved: None,
            query_text: None,
            params_json: Some(params_json),
            caller,
            provider: None,
            provider_model: None,
            result_ids: None,
            result_scores: None,
            latency,
        },
    );

    Ok(ContextOutcome { pack })
}
