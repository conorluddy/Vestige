//! `vestige_expand` tool — fetches a single memory at a chosen representation
//! depth. Delegates to `vestige_engine::context::expand_memory` which is the
//! single trace-write site for expand calls.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use vestige_core::{project_detail, MemoryId, RepresentationDepth};
use vestige_engine::context::expand_memory;
use vestige_engine::Caller;

use crate::server::{err, ok_json, VestigeServer};

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

#[tool_router(router = expand_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(description = "Expand a memory at a chosen representation depth: \
                          one_liner | summary | compressed | full. \
                          Returns the title, type, depth, and content.")]
    pub async fn vestige_expand(
        &self,
        Parameters(p): Parameters<ExpandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let id = MemoryId::from_str(&p.memory_id)
            .map_err(|e| err("INVALID_ID", e.to_string(), false))?;
        let depth = RepresentationDepth::from_str(&p.depth)
            .map_err(|e| err("INVALID_DEPTH", e.to_string(), false))?;

        // Scope check before we call into the engine.
        // get_memory is light; we do it here to surface OUT_OF_SCOPE before
        // the engine allocates its trace row.
        let fetched_check = inner
            .store
            .get_memory(&id)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
            .ok_or_else(|| err("MEMORY_NOT_FOUND", id.to_string(), false))?;
        if fetched_check.memory.project_id != inner.project_id {
            return Err(err(
                "OUT_OF_SCOPE",
                "memory belongs to another project",
                false,
            ));
        }

        // Delegate to the engine — single trace-write site for expand calls.
        let outcome = expand_memory(&inner.store, &inner.project_id, &id, depth, Caller::Mcp)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;

        let detail = project_detail(&outcome.fetched);
        let payload = serde_json::json!({
            "id": detail.card.id,
            "type": detail.card.r#type,
            "title": detail.card.title,
            "depth": depth.as_str(),
            "content": outcome.content,
        });
        ok_json(&payload)
    }
}
