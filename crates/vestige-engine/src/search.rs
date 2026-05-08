//! Hybrid search orchestration — single source of truth for all three retrieval
//! modes. Both `vestige-cli` and `vestige-mcp` delegate here so the fallback
//! logic, score normalisation, and candidate merging are never duplicated.
//!
//! # Mode summary
//!
//! | Mode | Requires | Fallback |
//! |------|----------|---------|
//! | Lexical | FTS5 index (always available) | — |
//! | Semantic | Active embeddings + matching provider | Hard error (`EngineError`) |
//! | Hybrid | Active embeddings + matching provider | Lexical + warning |
//!
//! Reuses ranking and normalisation primitives from
//! `vestige_core::memory::{search, scoring}`.

use std::collections::HashSet;

use serde::Serialize;

use vestige_core::{
    merge_hits, normalise_cosine, normalise_fts, project_card, rank_hits, sanitize_fts_query,
    HybridOpts, HybridScore, MemoryId, MemoryType, ProjectId, ScoredCard, SearchFilter, SearchHit,
    SearchMode, SemanticHit,
};
use vestige_embed::EmbeddingProvider;
use vestige_store::{EmbeddingStatus, Store, VectorFilter};

#[allow(unused_imports)] // referenced by intra-doc-links
use crate::error::EngineError;
use crate::error::Result;
use crate::trace::{
    elapsed_since, search_params_json, start_timer, write_trace, Caller, TraceKind, TracePayload,
};

// === TYPES ===

/// Return value for all three search variants.
///
/// Callers should always inspect `effective_mode` — it may differ from the
/// requested mode when `Hybrid` falls back to `Lexical` because:
/// - no embeddings have been generated yet (`vestige embed --all` has not
///   been run), or
/// - the stored embeddings were produced by a different provider/model/
///   dimensions than the current runtime configuration.
///
/// When a fallback occurs, a human-readable explanation is appended to
/// `warnings`; an empty `warnings` vec means the requested mode ran as-is.
#[derive(Debug, Clone, Serialize)]
pub struct HybridOutcome {
    /// Ranked results, compact cards (handle + one_liner + score).
    pub scored: Vec<ScoredCard>,
    /// Non-fatal messages for the caller to surface to the user or agent.
    /// Populated on fallback or when the query was sanitised to empty.
    pub warnings: Vec<String>,
    /// The mode that actually ran. May differ from the requested mode; see
    /// the struct-level doc for when that happens.
    pub effective_mode: SearchMode,
}

// === PUBLIC API ===

/// FTS5 keyword search (BM25).
///
/// Sanitises the query via [`sanitize_fts_query`] before handing it to the
/// store — FTS5 special characters are stripped per-token so the caller does
/// not need to pre-escape input.
///
/// An empty or whitespace-only query returns an empty [`HybridOutcome`]
/// without a warning. The caller decides whether to surface that fact to
/// the user or agent.
///
/// `caller` identifies the surface that initiated this recall (CLI or MCP).
/// One `query_events` row is written after the search completes; a write
/// failure is logged but never propagated (PRD §10.5).
///
/// # Errors
///
/// Returns [`EngineError::Store`] on SQLite failure.
pub fn search_lexical(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    caller: Caller,
) -> Result<HybridOutcome> {
    let t0 = start_timer();
    let cleaned = sanitize_fts_query(query);
    let scored = if cleaned.is_empty() {
        Vec::new()
    } else {
        let hits = store.search_memories(
            project_id,
            &cleaned,
            &SearchFilter {
                r#type: type_filter,
                limit: Some(limit),
                ..Default::default()
            },
        )?;
        rank_hits(hits)
    };
    let latency = elapsed_since(t0);

    let result_ids: Vec<MemoryId> = scored.iter().map(|c| c.card.id.clone()).collect();
    let result_scores: Vec<f64> = scored.iter().map(|c| c.score).collect();
    let type_filter_str = type_filter.map(|t| t.as_str().to_string());
    let params_json = search_params_json(limit, type_filter_str.as_deref());

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Search,
            mode_requested: Some(SearchMode::Lexical),
            mode_resolved: Some(SearchMode::Lexical),
            query_text: Some(query),
            params_json: Some(params_json),
            caller,
            provider: None,
            provider_model: None,
            result_ids: Some(&result_ids),
            result_scores: Some(&result_scores),
            latency,
        },
    );

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Lexical,
    })
}

/// Vector nearest-neighbour search (cosine similarity).
///
/// Embeds the query string using `provider`, then queries the store's vector
/// index for the `limit` nearest neighbours, hydrating each result into a
/// [`ScoredCard`].
///
/// **No fallback.** When no embeddings exist for the project this function
/// returns an empty result with a warning — agents can act on the warning by
/// running `vestige embed --all` and retrying. The choice not to hard-error
/// matches the pattern in MCP's semantic tool path (it checks first and
/// surfaces `EMBEDDINGS_UNAVAILABLE` before calling here).
///
/// `caller` identifies the surface that initiated this recall (CLI or MCP).
/// One `query_events` row is written after the search completes; a write
/// failure is logged but never propagated (PRD §10.5). Provider and model
/// are recorded when the search runs; they are null only for lexical.
///
/// # Errors
///
/// Returns [`EngineError::Embed`] when the provider fails to embed the query,
/// or [`EngineError::Store`] on SQLite failure.
pub fn search_semantic(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
    caller: Caller,
) -> Result<HybridOutcome> {
    let t0 = start_timer();
    let status = store.embedding_status(project_id)?;
    if status.embedded_representations == 0 {
        let latency = elapsed_since(t0);
        let type_filter_str = type_filter.map(|t| t.as_str().to_string());
        let params_json = search_params_json(limit, type_filter_str.as_deref());
        write_trace(
            store,
            &TracePayload {
                project_id,
                kind: TraceKind::Search,
                mode_requested: Some(SearchMode::Semantic),
                mode_resolved: Some(SearchMode::Semantic),
                query_text: Some(query),
                params_json: Some(params_json),
                caller,
                provider: Some(provider.provider_name()),
                provider_model: Some(provider.model_name()),
                result_ids: Some(&[]),
                result_scores: Some(&[]),
                latency,
            },
        );
        return Ok(HybridOutcome {
            scored: vec![],
            warnings: vec!["no embeddings; run `vestige embed --all` first".to_string()],
            effective_mode: SearchMode::Semantic,
        });
    }

    let query_vec = provider.embed(query)?;
    let filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let raw_hits = store.nearest_neighbours(project_id, &query_vec, limit, &filter)?;

    let mut scored = Vec::with_capacity(raw_hits.len());
    for hit in &raw_hits {
        if let Some(fetched) = store.get_memory(&hit.memory_id)? {
            let similarity = hit.similarity.clamp(0.0, 1.0);
            // Semantic-only path: rank score IS cosine similarity, so the
            // diagnostic mirrors that — `vector` and `total` both equal the
            // displayed score, other components are zero. PRD §11.3 / §19.4.
            scored.push(ScoredCard {
                card: project_card(&fetched),
                score: similarity,
                score_parts: Some(HybridScore {
                    fts: 0.0,
                    vector: similarity,
                    importance: 0.0,
                    type_boost: 0.0,
                    total: similarity,
                }),
            });
        }
    }
    let latency = elapsed_since(t0);

    let result_ids: Vec<MemoryId> = scored.iter().map(|c| c.card.id.clone()).collect();
    let result_scores: Vec<f64> = scored.iter().map(|c| c.score).collect();
    let type_filter_str = type_filter.map(|t| t.as_str().to_string());
    let params_json = search_params_json(limit, type_filter_str.as_deref());

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Search,
            mode_requested: Some(SearchMode::Semantic),
            mode_resolved: Some(SearchMode::Semantic),
            query_text: Some(query),
            params_json: Some(params_json),
            caller,
            provider: Some(provider.provider_name()),
            provider_model: Some(provider.model_name()),
            result_ids: Some(&result_ids),
            result_scores: Some(&result_scores),
            latency,
        },
    );

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Semantic,
    })
}

/// Merged lexical + semantic search.
///
/// # Fallback chain
///
/// 1. If no embeddings exist for the project, or the stored provider/model/
///    dimensions don't match the runtime provider, this function **falls back
///    to lexical search** and records a human-readable explanation in
///    `HybridOutcome::warnings`. `effective_mode` is set to `Lexical` so the
///    caller / agent knows what actually ran.
/// 2. When both legs are available, each leg over-fetches by a fixed
///    multiplier (with a floor) so the merger has a wide enough candidate set. Results are deduplicated
///    by [`MemoryId`], FTS BM25 scores and cosine similarities are
///    independently normalised to [0, 1], and [`merge_hits`] combines them
///    using the weights in [`HybridOpts::default`].
///
/// `caller` identifies the surface that initiated this recall (CLI or MCP).
/// One `query_events` row is written after the search completes; a write
/// failure is logged but never propagated (PRD §10.5). Provider and model
/// are recorded even on fallback, so the trace shows what was configured.
///
/// # Errors
///
/// Returns [`EngineError::Embed`] when the provider fails to embed the query,
/// or [`EngineError::Store`] on SQLite failure. Provider mismatch and missing
/// embeddings are surfaced as warnings, not errors.
pub fn search_hybrid(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
    caller: Caller,
) -> Result<HybridOutcome> {
    let t0 = start_timer();
    let status = store.embedding_status(project_id)?;
    let mismatch = provider_mismatch_message(&status, provider);

    if status.embedded_representations == 0 || mismatch.is_some() {
        let warning = match mismatch {
            Some(msg) => format!("hybrid falling back to lexical: {msg}"),
            None => "hybrid falling back to lexical: no embeddings (run `vestige embed --all` to enable semantic recall)".to_string(),
        };
        // Delegate to lexical for the actual results, then overlay mode + warning.
        // We do NOT call search_lexical here because that would write a second
        // trace row with Lexical mode. Instead, execute the FTS query inline
        // and write a single Hybrid→Lexical fallback trace.
        let cleaned = sanitize_fts_query(query);
        let fallback_scored = if cleaned.is_empty() {
            Vec::new()
        } else {
            let hits = store.search_memories(
                project_id,
                &cleaned,
                &SearchFilter {
                    r#type: type_filter,
                    limit: Some(limit),
                    ..Default::default()
                },
            )?;
            rank_hits(hits)
        };
        let latency = elapsed_since(t0);

        let result_ids: Vec<MemoryId> = fallback_scored.iter().map(|c| c.card.id.clone()).collect();
        let result_scores: Vec<f64> = fallback_scored.iter().map(|c| c.score).collect();
        let type_filter_str = type_filter.map(|t| t.as_str().to_string());
        let params_json = search_params_json(limit, type_filter_str.as_deref());

        write_trace(
            store,
            &TracePayload {
                project_id,
                kind: TraceKind::Search,
                mode_requested: Some(SearchMode::Hybrid),
                mode_resolved: Some(SearchMode::Lexical),
                query_text: Some(query),
                params_json: Some(params_json),
                caller,
                provider: Some(provider.provider_name()),
                provider_model: Some(provider.model_name()),
                result_ids: Some(&result_ids),
                result_scores: Some(&result_scores),
                latency,
            },
        );

        return Ok(HybridOutcome {
            scored: fallback_scored,
            warnings: vec![warning],
            effective_mode: SearchMode::Lexical,
        });
    }

    let per_leg = hybrid_per_leg_limit(limit);

    // --- Lexical leg ---
    let cleaned = sanitize_fts_query(query);
    let lexical_hits: Vec<SearchHit> = if cleaned.is_empty() {
        Vec::new()
    } else {
        store.search_memories(
            project_id,
            &cleaned,
            &SearchFilter {
                r#type: type_filter,
                limit: Some(per_leg),
                ..Default::default()
            },
        )?
    };

    // --- Semantic leg ---
    let query_vec = provider.embed(query)?;
    let vector_filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let vector_raw = store.nearest_neighbours(project_id, &query_vec, per_leg, &vector_filter)?;

    let semantic_hits: Vec<SemanticHit> = vector_raw
        .iter()
        .map(|h| SemanticHit {
            memory_id: h.memory_id.clone(),
            representation_type: h.representation_type.clone(),
            similarity: h.similarity,
        })
        .collect();

    // Build the unified candidate set: lexical results + semantic-only hydrated hits.
    // Use HashSet<MemoryId> (typed newtype, not HashSet<String>) for deduplication.
    let mut seen_ids: HashSet<MemoryId> = lexical_hits
        .iter()
        .map(|h| h.fetched.memory.id.clone())
        .collect();
    let mut candidates: Vec<SearchHit> = lexical_hits.clone();

    for sem_hit in &semantic_hits {
        if !seen_ids.insert(sem_hit.memory_id.clone()) {
            continue;
        }
        if let Some(fetched) = store.get_memory(&sem_hit.memory_id)? {
            candidates.push(SearchHit { fetched, bm25: 0.0 });
        }
    }

    let fts_scores = normalise_fts(&lexical_hits);
    let vector_scores = normalise_cosine(&semantic_hits);
    let opts = HybridOpts {
        limit,
        ..HybridOpts::default()
    };
    let scored = merge_hits(candidates, &fts_scores, &vector_scores, &opts);
    let latency = elapsed_since(t0);

    let result_ids: Vec<MemoryId> = scored.iter().map(|c| c.card.id.clone()).collect();
    let result_scores: Vec<f64> = scored.iter().map(|c| c.score).collect();
    let type_filter_str = type_filter.map(|t| t.as_str().to_string());
    let params_json = search_params_json(limit, type_filter_str.as_deref());

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Search,
            mode_requested: Some(SearchMode::Hybrid),
            mode_resolved: Some(SearchMode::Hybrid),
            query_text: Some(query),
            params_json: Some(params_json),
            caller,
            provider: Some(provider.provider_name()),
            provider_model: Some(provider.model_name()),
            result_ids: Some(&result_ids),
            result_scores: Some(&result_scores),
            latency,
        },
    );

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Hybrid,
    })
}

// === PRIVATE HELPERS ===

/// Over-fetch factor applied to each leg of a hybrid search before merging.
///
/// Deduplication and weighted re-ranking require more raw candidates than the
/// final `limit`. Each leg retrieves `limit * HYBRID_OVERFETCH_MULTIPLIER`
/// results (minimum [`HYBRID_OVERFETCH_FLOOR`]).
const HYBRID_OVERFETCH_MULTIPLIER: u32 = 4;

/// Minimum per-leg candidate count regardless of the requested `limit`.
const HYBRID_OVERFETCH_FLOOR: u32 = 32;

/// Compute how many results each leg should retrieve before merging.
fn hybrid_per_leg_limit(limit: u32) -> u32 {
    limit
        .saturating_mul(HYBRID_OVERFETCH_MULTIPLIER)
        .max(HYBRID_OVERFETCH_FLOOR)
}

/// Compare the on-disk dominant provider against the runtime provider.
///
/// Returns `Some(message)` when the stored provider/model/dimensions don't
/// match the runtime configuration — the silent-empty-results trap. Returns
/// `None` when embeddings are absent (that's a separate "unavailable" case)
/// or when everything matches.
///
/// Shared with `vestige-mcp`'s `vestige_search` tool — both layers must surface
/// identical wording when they detect the mismatch, so this is the single
/// source of truth.
pub fn provider_mismatch_message(
    status: &EmbeddingStatus,
    provider: &dyn EmbeddingProvider,
) -> Option<String> {
    // No embeddings yet — the caller already handles that as "unavailable".
    let stored_provider = status.provider.as_deref()?;
    let stored_model = status.model.as_deref().unwrap_or("?");
    let stored_dims = status.dimensions.unwrap_or(0);

    let runtime_provider = provider.provider_name();
    let runtime_model = provider.model_name();
    let runtime_dims = provider.dimensions();

    if stored_provider == runtime_provider
        && stored_model == runtime_model
        && stored_dims == runtime_dims
    {
        return None;
    }

    Some(format!(
        "project embedded with `{stored_provider}`/{stored_model}/{stored_dims}d, \
         configured for `{runtime_provider}`/{runtime_model}/{runtime_dims}d — \
         run `vestige embed --all` to re-embed under the configured provider"
    ))
}
