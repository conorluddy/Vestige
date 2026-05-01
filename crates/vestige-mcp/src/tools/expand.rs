//! `vestige_expand` tool — fetches a single memory at a chosen representation
//! depth. Adapts `store.get_memory` + `project_detail` + `pick_representation`
//! from `vestige-core`.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use vestige_core::{project_detail, MemoryId, RepresentationDepth};

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
}
