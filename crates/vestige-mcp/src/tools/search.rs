//! `vestige_search` tool — lexical (BM25), semantic (cosine), or hybrid search
//! over project memory. Adapts `search_memories` / `nearest_neighbours` from
//! `vestige-store` and normalisation / ranking helpers from `vestige-core`.

use std::collections::HashSet;
use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_config::EmbeddingsConfigSection;
use vestige_core::{
    merge_hits, normalise_cosine, normalise_fts, project_card, rank_hits, resolve_default_mode,
    sanitize_fts_query, HybridOpts, MemoryId, MemoryType, ScoredCard, SearchFilter, SearchHit,
    SearchMode, SemanticHit,
};
use vestige_embed::{build_provider, EmbeddingProvider, EmbeddingsConfig};
use vestige_store::VectorFilter;

use crate::server::{err, ok_json, Inner, VestigeServer};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Free-text query. FTS5 special characters are stripped per token.
    pub query: String,
    /// Search mode: `"lexical"` (BM25, default — always available),
    /// `"semantic"` (cosine over embeddings; requires `vestige embed --all` first),
    /// or `"hybrid"` (merged with score breakdown; falls back to lexical when no
    /// embeddings exist and adds a warning to the response).
    #[serde(default)]
    pub mode: Option<String>,
    /// Maximum results to return. Default 8.
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    /// Filter by memory type: `"decision"` | `"note"` | `"observation"` | etc.
    #[serde(default)]
    pub r#type: Option<String>,
    /// When `true`, each result includes a `score_parts` object with component
    /// scores (`fts`, `vector`, `importance`, `type_boost`, `total`). Automatically
    /// included for `hybrid` mode. Ignored for `lexical` (always `null`).
    #[serde(default)]
    pub include_score_parts: Option<bool>,
}

fn default_search_limit() -> u32 {
    8
}

#[tool_router(router = search_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Search project memory. Three modes: lexical (BM25 over text, default — \
                          always available), semantic (cosine over embeddings; requires \
                          `vestige embed --all` first), hybrid (merged, with score breakdown; \
                          falls back to lexical with a warning when no embeddings exist). \
                          Returns compact memory cards; use vestige_expand for full content."
    )]
    pub async fn vestige_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        // Explicit request param takes priority; config default is next; Lexical is the fallback.
        // A bad request param returns INVALID_MODE; a bad config value returns INVALID_CONFIG.
        if let Some(ref mode_str) = p.mode {
            SearchMode::from_str(mode_str)
                .map_err(|e| err("INVALID_MODE", e.to_string(), false))?;
        }
        let config_default = inner
            .config
            .search
            .as_ref()
            .and_then(|s| s.default_mode.as_deref());
        let mode = resolve_default_mode(p.mode.as_deref(), config_default).map_err(|e| {
            err(
                "INVALID_CONFIG",
                format!("invalid [search] default_mode: {e}"),
                false,
            )
        })?;

        let type_filter = p
            .r#type
            .as_deref()
            .map(MemoryType::from_str)
            .transpose()
            .map_err(|e| err("INVALID_TYPE", e.to_string(), false))?;

        match mode {
            SearchMode::Lexical => search_lexical(&inner, &p.query, p.limit, type_filter),
            SearchMode::Semantic => search_semantic(&inner, &p.query, p.limit, type_filter),
            SearchMode::Hybrid => search_hybrid(
                &inner,
                &p.query,
                p.limit,
                type_filter,
                p.include_score_parts,
            ),
        }
    }
}

// ========================================
// === SEARCH HELPERS ===
// ========================================

/// Over-fetch each leg of hybrid search by this factor before merging — the
/// merger drops duplicates and re-ranks, so the per-leg `limit` is
/// `max(limit * MULTIPLIER, FLOOR)`.
const HYBRID_OVERFETCH_MULTIPLIER: u32 = 4;
const HYBRID_OVERFETCH_FLOOR: u32 = 32;

fn hybrid_per_leg_limit(limit: u32) -> u32 {
    limit
        .saturating_mul(HYBRID_OVERFETCH_MULTIPLIER)
        .max(HYBRID_OVERFETCH_FLOOR)
}

/// Result envelope for all three search modes (PRD §13.3).
#[derive(Debug, Serialize)]
struct SearchEnvelope<'a> {
    mode: &'static str,
    results: &'a [ScoredCard],
    warnings: Vec<String>,
}

fn search_lexical(
    inner: &Inner,
    query: &str,
    limit: u32,
    type_filter: Option<MemoryType>,
) -> Result<CallToolResult, ErrorData> {
    let cleaned = sanitize_fts_query(query);
    let scored: Vec<ScoredCard> = if cleaned.is_empty() {
        Vec::new()
    } else {
        let hits = inner
            .store
            .search_memories(
                &inner.project_id,
                &cleaned,
                &SearchFilter {
                    r#type: type_filter,
                    limit: Some(limit),
                    ..Default::default()
                },
            )
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
        rank_hits(hits)
    };
    let envelope = SearchEnvelope {
        mode: "lexical",
        results: &scored,
        warnings: vec![],
    };
    ok_json(&envelope)
}

fn search_semantic(
    inner: &Inner,
    query: &str,
    limit: u32,
    type_filter: Option<MemoryType>,
) -> Result<CallToolResult, ErrorData> {
    // Check embedding coverage before querying.
    let status = inner
        .store
        .embedding_status(&inner.project_id)
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
    if status.embedded_representations == 0 {
        return Err(err(
            "EMBEDDINGS_UNAVAILABLE",
            "No embeddings found for this project. Run `vestige embed --all` first.",
            false,
        ));
    }

    let provider = build_configured_provider(inner)?;
    if let Some(msg) = provider_mismatch_message(&status, provider.as_ref()) {
        return Err(err("EMBEDDINGS_UNAVAILABLE", msg, false));
    }
    let query_vec = provider
        .embed(query)
        .map_err(|e| err("EMBED_FAILED", e.to_string(), false))?;
    let filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let raw_hits = inner
        .store
        .nearest_neighbours(&inner.project_id, &query_vec, limit, &filter)
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;

    let mut scored: Vec<ScoredCard> = Vec::with_capacity(raw_hits.len());
    for hit in &raw_hits {
        if let Some(fetched) = inner
            .store
            .get_memory(&hit.memory_id)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
        {
            let similarity = hit.similarity.clamp(0.0, 1.0);
            scored.push(ScoredCard {
                card: project_card(&fetched),
                score: similarity,
                score_parts: None,
            });
        }
    }

    let envelope = SearchEnvelope {
        mode: "semantic",
        results: &scored,
        warnings: vec![],
    };
    ok_json(&envelope)
}

fn search_hybrid(
    inner: &Inner,
    query: &str,
    limit: u32,
    type_filter: Option<MemoryType>,
    include_score_parts: Option<bool>,
) -> Result<CallToolResult, ErrorData> {
    // Hybrid always populates score_parts (PRD §13.3); the param is accepted
    // for symmetry with semantic mode but ignored here.
    let _ = include_score_parts;

    // Check embedding coverage; fall back to lexical with a warning if absent.
    let status = inner
        .store
        .embedding_status(&inner.project_id)
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;

    let configured_provider = build_configured_provider(inner)?;
    let mismatch = provider_mismatch_message(&status, configured_provider.as_ref());
    if status.embedded_representations == 0 || mismatch.is_some() {
        let warning = match mismatch {
            Some(msg) => format!("hybrid falling back to lexical: {msg}"),
            None => "embeddings unavailable; hybrid falling back to lexical (run `vestige embed --all` to enable semantic recall)".to_string(),
        };
        let cleaned = sanitize_fts_query(query);
        let scored: Vec<ScoredCard> = if cleaned.is_empty() {
            Vec::new()
        } else {
            let hits = inner
                .store
                .search_memories(
                    &inner.project_id,
                    &cleaned,
                    &SearchFilter {
                        r#type: type_filter,
                        limit: Some(limit),
                        ..Default::default()
                    },
                )
                .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
            rank_hits(hits)
        };
        let envelope = SearchEnvelope {
            mode: "hybrid",
            results: &scored,
            warnings: vec![warning],
        };
        return ok_json(&envelope);
    }

    // --- Lexical leg ---
    let cleaned = sanitize_fts_query(query);
    let lexical_hits: Vec<SearchHit> = if cleaned.is_empty() {
        Vec::new()
    } else {
        inner
            .store
            .search_memories(
                &inner.project_id,
                &cleaned,
                &SearchFilter {
                    r#type: type_filter,
                    // Over-fetch for the merge; core applies limit after.
                    limit: Some(hybrid_per_leg_limit(limit)),
                    ..Default::default()
                },
            )
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
    };

    // --- Semantic leg ---
    let provider = configured_provider;
    let query_vec = provider
        .embed(query)
        .map_err(|e| err("EMBED_FAILED", e.to_string(), false))?;
    let vector_filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let vector_raw = inner
        .store
        .nearest_neighbours(
            &inner.project_id,
            &query_vec,
            hybrid_per_leg_limit(limit),
            &vector_filter,
        )
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;

    // Map VectorHit → SemanticHit (core-side type).
    let semantic_hits: Vec<SemanticHit> = vector_raw
        .iter()
        .map(|h| SemanticHit {
            memory_id: h.memory_id.clone(),
            representation_type: h.representation_type.clone(),
            similarity: h.similarity,
        })
        .collect();

    // Build unified candidate set: lexical hits + semantic-only hydrated hits.
    let mut seen_ids: HashSet<MemoryId> = lexical_hits
        .iter()
        .map(|h| h.fetched.memory.id.clone())
        .collect();
    let mut candidates: Vec<SearchHit> = lexical_hits.clone();

    for sem_hit in &semantic_hits {
        if !seen_ids.insert(sem_hit.memory_id.clone()) {
            continue;
        }
        if let Some(fetched) = inner
            .store
            .get_memory(&sem_hit.memory_id)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
        {
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

    let envelope = SearchEnvelope {
        mode: "hybrid",
        results: &scored,
        warnings: vec![],
    };
    ok_json(&envelope)
}

/// Construct an embedding provider from the project's typed `[embeddings]`
/// config section, defaulting to `"fake"` when absent.
fn build_configured_provider(inner: &Inner) -> Result<Box<dyn EmbeddingProvider>, ErrorData> {
    let cfg = embeddings_config_from_section(inner.config.embeddings.as_ref());
    build_provider(&cfg).map_err(|e| err("PROVIDER_INIT_FAILED", e.to_string(), false))
}

/// Map a typed `[embeddings]` config section onto the runtime `EmbeddingsConfig`.
/// Mirrors `vestige_cli::context::embeddings_config_from_section` — duplicated
/// here to avoid `vestige-mcp → vestige-cli` (cli is a binary, not a library).
fn embeddings_config_from_section(section: Option<&EmbeddingsConfigSection>) -> EmbeddingsConfig {
    match section {
        Some(s) => EmbeddingsConfig {
            provider: s.provider.clone().unwrap_or_else(|| "fake".to_string()),
            model: s.model.clone(),
            dimensions: s.dimensions,
        },
        None => EmbeddingsConfig {
            provider: "fake".to_string(),
            model: None,
            dimensions: None,
        },
    }
}

/// Compare what's in the store against what the configured provider would query.
///
/// Returns `Some(message)` when the dominant on-disk provider/dimensions don't
/// match the runtime — the silent-empty-results trap that bit V0.1.
fn provider_mismatch_message(
    status: &vestige_store::EmbeddingStatus,
    provider: &dyn EmbeddingProvider,
) -> Option<String> {
    // No embeddings yet — that's the "embeddings_unavailable" case the caller
    // already handled; nothing to mismatch against.
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
         server configured for `{runtime_provider}`/{runtime_model}/{runtime_dims}d — \
         run `vestige embed --all` to re-embed under the configured provider"
    ))
}
