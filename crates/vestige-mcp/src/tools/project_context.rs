//! `vestige_get_project_context` tool — returns a budget-bounded context pack
//! for the current project. Adapts `build_context_pack` (shared in `tools/mod.rs`).

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use crate::server::{ok_json, VestigeServer};
use crate::tools::build_context_pack;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProjectContextParams {
    #[serde(default = "default_budget_tokens")]
    pub budget_tokens: usize,
    #[serde(default = "default_per_section")]
    pub per_section: u32,
}

fn default_budget_tokens() -> usize {
    1200
}

fn default_per_section() -> u32 {
    8
}

#[tool_router(router = project_context_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Return a budget-bounded context pack for the current project. \
                          Sections: project summary, current decisions, open questions, \
                          recent important memories."
    )]
    pub async fn vestige_get_project_context(
        &self,
        Parameters(p): Parameters<ProjectContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;
        let pack = build_context_pack(&inner, p.per_section, p.budget_tokens)?;
        ok_json(&pack)
    }
}
