//! `vestige_get_project_context` tool — returns a budget-bounded context pack
//! for the current project. Delegates to `vestige_engine::context::get_project_context`
//! which is the single trace-write site for context calls.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use vestige_config::traces_config_for;
use vestige_engine::context::get_project_context;
use vestige_engine::Caller;

use crate::server::{err, ok_json, VestigeServer};

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
        let traces_cfg = traces_config_for(inner.config.traces.as_ref());
        let outcome = get_project_context(
            &inner.store,
            &inner.project_id,
            &inner.config.project_name,
            p.per_section,
            p.budget_tokens,
            Caller::Mcp,
            &traces_cfg,
        )
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
        ok_json(&outcome.pack)
    }
}
