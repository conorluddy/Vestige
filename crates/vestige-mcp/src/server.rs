//! Six MCP tools (PRD §13.2). Thin wrappers over `vestige-core`.

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

use vestige_config::VestigeConfig;
use vestige_core::{
    build_bundle, build_pack, project_card, project_detail, rank_hits, sanitize_fts_query,
    ContextOptions, ContextSources, ListFilter, MemoryId, MemoryType, NewMemory, NewSource,
    ProjectId, RepresentationDepth, ScoredCard, SearchFilter, SOURCE_SNIPPET_MAX_BYTES,
};
use vestige_store::Store;

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
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub r#type: Option<String>,
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
        description = "Search project memory (FTS5 over all representations). \
                          Returns compact memory cards with composite ranking scores. \
                          Caller expands selected memories via vestige_expand."
    )]
    async fn vestige_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let r#type = match p.r#type.as_deref() {
            Some(s) => Some(
                MemoryType::from_str(s).map_err(|e| err("INVALID_TYPE", e.to_string(), false))?,
            ),
            None => None,
        };
        let cleaned = sanitize_fts_query(&p.query);
        let scored: Vec<ScoredCard> = if cleaned.is_empty() {
            Vec::new()
        } else {
            let hits = inner
                .store
                .search_memories(
                    &inner.project_id,
                    &cleaned,
                    &SearchFilter {
                        r#type,
                        limit: Some(p.limit),
                    },
                )
                .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
            rank_hits(hits)
        };
        ok_json(&scored)
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
