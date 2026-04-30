//! Six MCP tools (PRD §13.2). Thin wrappers over `vestige-core`.

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use vestige_config::{EmbeddingsConfigSection, VestigeConfig};
use vestige_core::{
    build_bundle, build_pack, merge_hits, normalise_cosine, normalise_fts, project_card,
    project_detail, rank_hits, sanitize_fts_query, ContextOptions, ContextSources, HybridOpts,
    ListFilter, MemoryId, MemoryType, NewMemory, NewSource, ProjectId, RepresentationDepth,
    ScoredCard, SearchFilter, SearchHit, SearchMode, SemanticHit, SOURCE_SNIPPET_MAX_BYTES,
};
use vestige_embed::{build_provider, EmbeddingProvider, EmbeddingsConfig};
use vestige_store::{Store, VectorFilter};

#[derive(Clone)]
pub struct VestigeServer {
    inner: Arc<Mutex<Inner>>,
    tool_router: ToolRouter<Self>,
}

struct Inner {
    store: Store,
    config: VestigeConfig,
    project_id: ProjectId,
    read_only: bool,
}

impl VestigeServer {
    pub fn new(
        store: Store,
        config: VestigeConfig,
        project_id: ProjectId,
        read_only: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                store,
                config,
                project_id,
                read_only,
            })),
            tool_router: Self::tool_router(),
        }
    }
}

// ========================================
// === TOOL PARAMETER SCHEMAS ===
// ========================================

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BootstrapParams {
    /// Maximum number of items to include in any list section.
    #[serde(default = "default_max_items")]
    pub max_items: u32,
}

fn default_max_items() -> u32 {
    8
}

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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExpandParams {
    pub memory_id: String,
    /// one_liner | summary | compressed | full
    #[serde(default = "default_depth")]
    pub depth: String,
}

fn default_depth() -> String {
    "summary".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProjectContextParams {
    #[serde(default = "default_budget")]
    pub budget_tokens: usize,
    #[serde(default = "default_per_section")]
    pub per_section: u32,
}

fn default_budget() -> usize {
    1200
}
fn default_per_section() -> u32 {
    8
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RecordObservationParams {
    pub content: String,
    #[serde(default = "default_obs_importance")]
    pub importance: f64,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub source_content: Option<String>,
}
fn default_obs_importance() -> f64 {
    0.5
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RecordDecisionParams {
    pub decision: String,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default = "default_dec_importance")]
    pub importance: f64,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub source_content: Option<String>,
}
fn default_dec_importance() -> f64 {
    0.7
}

// ========================================
// === MCP-FRIENDLY ERROR SHAPE ===
// ========================================

#[derive(Debug, Serialize)]
struct ToolErrorBody {
    code: &'static str,
    message: String,
    retryable: bool,
}

fn err(code: &'static str, message: impl Into<String>, retryable: bool) -> ErrorData {
    let body = ToolErrorBody {
        code,
        message: message.into(),
        retryable,
    };
    let json = serde_json::to_string(&body).unwrap_or_else(|_| format!("{{\"code\":\"{code}\"}}"));
    ErrorData::internal_error(json, None)
}

fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let json =
        serde_json::to_string(value).map_err(|e| err("SERIALIZE_FAILED", e.to_string(), false))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

// ========================================
// === TOOLS ===
// ========================================

#[tool_router]
impl VestigeServer {
    #[tool(
        description = "Return compact standing context for the current project: \
                          project name, summary, recent decisions, open questions, \
                          and recent important memories."
    )]
    async fn vestige_bootstrap(
        &self,
        Parameters(p): Parameters<BootstrapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let pack = build_context_pack(&inner, p.max_items, default_budget())?;
        ok_json(&pack)
    }

    #[tool(
        description = "Search project memory. Three modes: lexical (BM25 over text, default — \
                          always available), semantic (cosine over embeddings; requires \
                          `vestige embed --all` first), hybrid (merged, with score breakdown; \
                          falls back to lexical with a warning when no embeddings exist). \
                          Returns compact memory cards; use vestige_expand for full content."
    )]
    async fn vestige_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        let mode = p
            .mode
            .as_deref()
            .map(SearchMode::from_str)
            .transpose()
            .map_err(|e| err("INVALID_MODE", e.to_string(), false))?
            .unwrap_or(SearchMode::Lexical);

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

    #[tool(description = "Expand a memory at a chosen representation depth: \
                          one_liner | summary | compressed | full. \
                          Returns the title, type, depth, and content.")]
    async fn vestige_expand(
        &self,
        Parameters(p): Parameters<ExpandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let id = MemoryId::from_str(&p.memory_id)
            .map_err(|e| err("INVALID_ID", e.to_string(), false))?;
        let depth = RepresentationDepth::from_str(&p.depth)
            .map_err(|e| err("INVALID_DEPTH", e.to_string(), false))?;
        let fetched = inner
            .store
            .get_memory(&id)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
            .ok_or_else(|| err("MEMORY_NOT_FOUND", id.to_string(), false))?;
        if fetched.memory.project_id != inner.project_id {
            return Err(err(
                "OUT_OF_SCOPE",
                "memory belongs to another project",
                false,
            ));
        }
        let detail = project_detail(&fetched);
        let content = detail
            .representations
            .iter()
            .find(|(d, _)| *d == depth)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        let payload = serde_json::json!({
            "id": detail.card.id,
            "type": detail.card.r#type,
            "title": detail.card.title,
            "depth": depth.as_str(),
            "content": content,
        });
        ok_json(&payload)
    }

    #[tool(
        description = "Return a budget-bounded context pack for the current project. \
                          Sections: project summary, current decisions, open questions, \
                          recent important memories."
    )]
    async fn vestige_get_project_context(
        &self,
        Parameters(p): Parameters<ProjectContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let pack = build_context_pack(&inner, p.per_section, p.budget_tokens)?;
        ok_json(&pack)
    }

    #[tool(
        description = "Record a low-to-medium confidence project observation. \
                          Disabled when the server runs with --read-only."
    )]
    async fn vestige_record_observation(
        &self,
        Parameters(p): Parameters<RecordObservationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;
        if inner.read_only {
            return Err(err(
                "READ_ONLY",
                "MCP server is read-only; record_observation is disabled",
                false,
            ));
        }
        let card = capture(
            &mut inner,
            MemoryType::Observation,
            &p.content,
            p.importance,
            p.source_ref.as_deref(),
            p.source_content.as_deref(),
        )?;
        ok_json(&card)
    }

    #[tool(description = "Record an explicit project decision. \
                          Disabled when the server runs with --read-only.")]
    async fn vestige_record_decision(
        &self,
        Parameters(p): Parameters<RecordDecisionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;
        if inner.read_only {
            return Err(err(
                "READ_ONLY",
                "MCP server is read-only; record_decision is disabled",
                false,
            ));
        }
        let body = match p.rationale.as_deref() {
            Some(r) => format!("{}\n\nRationale: {}", p.decision, r),
            None => p.decision.clone(),
        };
        let card = capture(
            &mut inner,
            MemoryType::Decision,
            &body,
            p.importance,
            p.source_ref.as_deref(),
            p.source_content.as_deref(),
        )?;
        ok_json(&card)
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

// ========================================
// === SHARED HELPERS ===
// ========================================

fn build_context_pack(
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

fn list(
    inner: &Inner,
    r#type: Option<MemoryType>,
    limit: u32,
) -> Result<Vec<vestige_core::FetchedMemory>, ErrorData> {
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

fn capture(
    inner: &mut Inner,
    r#type: MemoryType,
    body: &str,
    importance: f64,
    source_ref: Option<&str>,
    source_content: Option<&str>,
) -> Result<vestige_core::MemoryCard, ErrorData> {
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
    let mut card = project_card(&vestige_core::FetchedMemory {
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

// ========================================
// === SERVER HANDLER ===
// ========================================

#[tool_handler]
impl ServerHandler for VestigeServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Vestige: repo-pinned memory layer for coding agents. \
                 Tools expose project memory operations; storage is local SQLite. \
                 Use vestige_get_project_context at the start of a session, \
                 vestige_search to find relevant memories, vestige_expand to read \
                 them at higher fidelity, and vestige_record_decision to capture \
                 new project decisions."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
