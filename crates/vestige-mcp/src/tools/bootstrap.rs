//! `vestige_bootstrap` tool — returns compact standing context for the current
//! project. Adapts `build_context_pack` (shared helper in `tools/mod.rs`).

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use crate::server::{ok_json, VestigeServer};
use crate::tools::{build_context_pack, default_budget};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BootstrapParams {
    /// Maximum number of items to include in any list section.
    #[serde(default = "default_max_items")]
    pub max_items: u32,
}

fn default_max_items() -> u32 {
    8
}

#[tool_router(router = bootstrap_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Return compact standing context for the current project: \
                          project name, summary, recent decisions, open questions, \
                          and recent important memories."
    )]
    pub async fn vestige_bootstrap(
        &self,
        Parameters(p): Parameters<BootstrapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let pack = build_context_pack(&inner, p.max_items, default_budget())?;
        ok_json(&pack)
    }
}
