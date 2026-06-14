//! MCP tool handlers. One file per tool, each owning its `*Params` schema,
//! its `#[tool]` handler, and any tool-local helpers. Per CLAUDE.md the
//! `vestige-mcp` crate is a thin adapter — orchestration lives in
//! `vestige-engine` (Wave 3 swaps the call sites).

pub mod bootstrap;
pub mod expand;
pub mod get_candidate;
pub mod list_candidates;
pub mod project_context;
pub mod propose_candidate;
pub mod record_decision;
pub mod record_observation;
pub mod scan_sessions;
pub mod search;
pub mod trace;

use rmcp::ErrorData;
use vestige_core::{
    build_bundle, build_pack, project_card, ContextOptions, ContextSources, FetchedMemory,
    ListFilter, MemoryCard, MemoryType, NewMemory, NewSource, SOURCE_SNIPPET_MAX_BYTES,
};

use crate::server::{err, Inner};

/// Default token budget for context packs; shared by bootstrap and get_project_context.
pub(crate) fn default_budget() -> usize {
    1200
}

// ========================================
// === SHARED TOOL HELPERS ===
// ========================================

/// Build a budget-bounded context pack; used by bootstrap and get_project_context.
pub(crate) fn build_context_pack(
    inner: &Inner,
    per_section: u32,
    budget_tokens: usize,
) -> Result<vestige_core::ContextPack, ErrorData> {
    let summary = inner
        .store
        .list_memories(
            &inner.project_id,
            &ListFilter {
                include_deleted: false,
                r#type: Some(MemoryType::ProjectSummary),
                limit: Some(1),
            },
        )
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
        .into_iter()
        .next();
    let decisions = list(inner, Some(MemoryType::Decision), per_section)?;
    let open_questions = list(inner, Some(MemoryType::OpenQuestion), per_section)?;
    let recent = list(inner, None, per_section)?;
    Ok(build_pack(
        ContextSources {
            project_name: inner.config.project_name.clone(),
            summary,
            decisions,
            open_questions,
            recent,
        },
        ContextOptions { budget_tokens },
    ))
}

/// List memories of an optional type up to `limit`; used by build_context_pack.
pub(crate) fn list(
    inner: &Inner,
    r#type: Option<MemoryType>,
    limit: u32,
) -> Result<Vec<FetchedMemory>, ErrorData> {
    inner
        .store
        .list_memories(
            &inner.project_id,
            &ListFilter {
                include_deleted: false,
                r#type,
                limit: Some(limit),
            },
        )
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))
}

/// Write a new memory to the store and return its card; used by record_observation
/// and record_decision.
pub(crate) fn capture(
    inner: &mut Inner,
    r#type: MemoryType,
    body: &str,
    importance: f64,
    source_ref: Option<&str>,
    source_content: Option<&str>,
) -> Result<MemoryCard, ErrorData> {
    let source = match (source_ref, source_content) {
        (None, None) => None,
        (r, c) => Some(NewSource {
            source_type: "mcp",
            source_ref: r,
            source_content: c,
        }),
    };
    let bundle = build_bundle(
        &inner.project_id,
        NewMemory {
            r#type,
            body,
            importance,
            source,
        },
    )
    .map_err(|e| match &e {
        vestige_core::CoreError::Validation(_) => err("VALIDATION", e.to_string(), false),
        _ => err("CORE_FAILED", e.to_string(), false),
    })?;
    let truncated = bundle.source.as_ref().map(|s| s.truncated).unwrap_or(false);
    inner
        .store
        .record_memory(&bundle)
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
    let mut card = project_card(&FetchedMemory {
        memory: bundle.memory,
        representations: bundle.representations,
        sources: vec![],
    });
    if truncated {
        // Surface truncation via a marker at the end of one_liner so the
        // agent sees it without changing the schema.
        card.one_liner.push_str(&format!(
            " (source truncated at {SOURCE_SNIPPET_MAX_BYTES} bytes)"
        ));
    }
    Ok(card)
}
