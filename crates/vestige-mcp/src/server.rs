//! MCP server struct, constructor, shared error helpers, and `ServerHandler`
//! impl. Tool handlers live in `crates/vestige-mcp/src/tools/` — one file per
//! tool, each with its own `#[tool_router(router = <name>_router)]` impl block.

use std::sync::Arc;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool_handler, ErrorData, ServerHandler,
};
use serde::Serialize;
use tokio::sync::Mutex;

use vestige_config::VestigeConfig;
use vestige_core::ProjectId;
use vestige_store::Store;

#[derive(Clone)]
pub struct VestigeServer {
    pub(crate) inner: Arc<Mutex<Inner>>,
    tool_router: ToolRouter<Self>,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) config: VestigeConfig,
    pub(crate) project_id: ProjectId,
    pub(crate) read_only: bool,
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
            tool_router: Self::bootstrap_router()
                + Self::search_router()
                + Self::expand_router()
                + Self::project_context_router()
                + Self::record_observation_router()
                + Self::record_decision_router()
                + Self::propose_candidate_router()
                + Self::list_candidates_router()
                + Self::get_candidate_router()
                + Self::trace_router(),
        }
    }
}

// ========================================
// === MCP-FRIENDLY ERROR SHAPE ===
// ========================================

#[derive(Debug, Serialize)]
pub(crate) struct ToolErrorBody {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

pub(crate) fn err(code: &'static str, message: impl Into<String>, retryable: bool) -> ErrorData {
    let body = ToolErrorBody {
        code,
        message: message.into(),
        retryable,
    };
    let json = serde_json::to_string(&body).unwrap_or_else(|_| format!("{{\"code\":\"{code}\"}}"));
    ErrorData::internal_error(json, None)
}

pub(crate) fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let json =
        serde_json::to_string(value).map_err(|e| err("SERIALIZE_FAILED", e.to_string(), false))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
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
