//! Hybrid search orchestration. Single source of truth for lexical / semantic /
//! hybrid retrieval; CLI and MCP call into here so the three legs aren't
//! duplicated three times. Reuses ranking + normalisation primitives from
//! `vestige_core::memory::{search, scoring}`.

use std::collections::HashSet;

use serde::Serialize;

use vestige_core::{
    merge_hits, normalise_cosine, normalise_fts, project_card, rank_hits, sanitize_fts_query,
    HybridOpts, MemoryId, MemoryType, ProjectId, ScoredCard, SearchFilter, SearchHit, SearchMode,
    SemanticHit,
};
use vestige_embed::EmbeddingProvider;
use vestige_store::{EmbeddingStatus, Store, VectorFilter};

use crate::error::Result;

// === TYPES ===

/// What every search variant returns.
///
/// `effective_mode` may differ from the caller's request when `Hybrid` falls
/// back to `Lexical` (no embeddings, or provider mismatch).
#[derive(Debug, Clone, Serialize)]
pub struct HybridOutcome {
    pub scored: Vec<ScoredCard>,
    pub warnings: Vec<String>,
    pub effective_mode: SearchMode,
}

// === PUBLIC API ===

/// FTS5 keyword search. Empty or whitespace-only query returns an empty result
/// without a warning — the caller decides whether to surface that to the user.
pub fn search_lexical(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
) -> Result<HybridOutcome> {
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

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Lexical,
    })
}

/// Vector nearest-neighbour search. When no embeddings exist, returns an empty
/// result with a warning rather than hard-erroring — agents can act on the
/// warning and retry after running `vestige embed --all`.
pub fn search_semantic(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
) -> Result<HybridOutcome> {
    let status = store.embedding_status(project_id)?;
    if status.embedded_representations == 0 {
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
            scored.push(ScoredCard {
                card: project_card(&fetched),
                score: similarity,
                score_parts: None,
            });
        }
    }

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Semantic,
    })
}

/// Merged lexical + semantic search.
///
/// Falls back to lexical (with a warning) when:
/// - No embeddings exist for the project, or
/// - The configured provider/model/dimensions don't match what's stored.
///
/// When both legs are available, over-fetches each by `hybrid_per_leg_limit`,
/// deduplicates the candidate set, and delegates to `merge_hits` for weighted
/// score combination.
pub fn search_hybrid(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
) -> Result<HybridOutcome> {
    let status = store.embedding_status(project_id)?;
    let mismatch = provider_mismatch_message(&status, provider);

    if status.embedded_representations == 0 || mismatch.is_some() {
        let warning = match mismatch {
            Some(msg) => format!("hybrid falling back to lexical: {msg}"),
            None => "hybrid falling back to lexical: no embeddings (run `vestige embed --all` to enable semantic recall)".to_string(),
        };
        // Delegate to lexical for the actual results, then overlay mode + warning.
        let lexical = search_lexical(store, project_id, query, type_filter, limit)?;
        return Ok(HybridOutcome {
            scored: lexical.scored,
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

    Ok(HybridOutcome {
        scored,
        warnings: vec![],
        effective_mode: SearchMode::Hybrid,
    })
}

// === PRIVATE HELPERS ===

/// Over-fetch factor for each leg of hybrid search before merging.
///
/// The merger deduplicates and re-ranks, so each leg retrieves more than the
/// final requested limit to give the merger enough candidates to work with.
const HYBRID_OVERFETCH_MULTIPLIER: u32 = 4;
const HYBRID_OVERFETCH_FLOOR: u32 = 32;

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
fn provider_mismatch_message(
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
