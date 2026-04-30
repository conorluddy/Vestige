//! `vestige_record_observation` tool — writes a low-to-medium confidence
//! project observation. Adapts `capture` (shared helper in `tools/mod.rs`).

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use vestige_core::MemoryType;

use crate::server::{err, ok_json, VestigeServer};
use crate::tools::capture;

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

#[tool_router(router = record_observation_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Record a low-to-medium confidence project observation. \
                          Disabled when the server runs with --read-only."
    )]
    pub async fn vestige_record_observation(
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
}
