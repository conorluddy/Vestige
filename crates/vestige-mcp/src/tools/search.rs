//! `vestige_search` tool — lexical (BM25), semantic (cosine), or hybrid search
//! over project memory. Delegates orchestration to `vestige_engine::search_*`.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_config::embeddings_config_for;
use vestige_core::{resolve_default_mode, MemoryType, ScoredCard, SearchMode};
use vestige_embed::{build_provider, EmbeddingProvider};
use vestige_engine::error::EngineError;

use crate::server::{err, ok_json, Inner, VestigeServer};

// === TYPES ===

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

/// Result envelope for all three search modes (PRD §13.3).
#[derive(Debug, Serialize)]
struct SearchEnvelope<'a> {
    mode: &'static str,
    results: &'a [ScoredCard],
    warnings: Vec<String>,
}

// === PUBLIC API ===

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

        let outcome = match mode {
            SearchMode::Lexical => vestige_engine::search::search_lexical(
                &inner.store,
                &inner.project_id,
                &p.query,
                type_filter,
                p.limit,
            )
            .map_err(engine_err_to_data)?,
            SearchMode::Semantic => {
                let provider = build_configured_provider(&inner)?;
                // Semantic mode has no fallback — surface unavailability as a
                // hard error so agents get a clear actionable code rather than
                // a silent empty result.
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
                if let Some(msg) =
                    vestige_engine::search::provider_mismatch_message(&status, provider.as_ref())
                {
                    return Err(err("EMBEDDINGS_UNAVAILABLE", msg, false));
                }
                vestige_engine::search::search_semantic(
                    &inner.store,
                    &inner.project_id,
                    &p.query,
                    type_filter,
                    p.limit,
                    provider.as_ref(),
                )
                .map_err(engine_err_to_data)?
            }
            SearchMode::Hybrid => {
                let provider = build_configured_provider(&inner)?;
                vestige_engine::search::search_hybrid(
                    &inner.store,
                    &inner.project_id,
                    &p.query,
                    type_filter,
                    p.limit,
                    provider.as_ref(),
                )
                .map_err(engine_err_to_data)?
            }
        };

        let envelope = SearchEnvelope {
            mode: outcome.effective_mode.as_str(),
            results: &outcome.scored,
            warnings: outcome.warnings,
        };
        ok_json(&envelope)
    }
}

// === PRIVATE HELPERS ===

/// Map `EngineError` variants to structured MCP `ErrorData` codes.
fn engine_err_to_data(e: EngineError) -> ErrorData {
    match e {
        EngineError::Store(_) => err("STORE_FAILED", e.to_string(), true),
        EngineError::Embed(_) => err("EMBED_FAILED", e.to_string(), false),
        EngineError::EmbeddingsUnavailable(_) => {
            err("EMBEDDINGS_UNAVAILABLE", e.to_string(), false)
        }
        // Candidate-specific errors are not reachable from the search tool, but
        // must be covered because `EngineError` is non-exhaustive in the future.
        EngineError::CandidateNotFound { .. } => err("CANDIDATE_NOT_FOUND", e.to_string(), false),
        EngineError::CandidateNotPending { .. } => {
            err("CANDIDATE_NOT_PENDING", e.to_string(), false)
        }
        EngineError::OutOfScope => err("OUT_OF_SCOPE", e.to_string(), false),
        EngineError::Validation { .. } => err("VALIDATION_ERROR", e.to_string(), false),
        EngineError::Core(_) => err("CORE_ERROR", e.to_string(), false),
    }
}

/// Construct an embedding provider from the project's typed `[embeddings]`
/// config section, defaulting to `"fake"` when absent.
fn build_configured_provider(inner: &Inner) -> Result<Box<dyn EmbeddingProvider>, ErrorData> {
    let cfg = embeddings_config_for(inner.config.embeddings.as_ref());
    build_provider(&cfg).map_err(|e| err("PROVIDER_INIT_FAILED", e.to_string(), false))
}

// Provider-mismatch detection lives in `vestige_engine::search::provider_mismatch_message`
// — that crate is the single source of truth for the wording used by both
// CLI and MCP search paths.
