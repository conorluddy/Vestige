//! `vestige_record_decision` tool — records an explicit project decision.
//! Adapts `capture` (shared helper in `tools/mod.rs`).

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

#[tool_router(router = record_decision_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(description = "Record an explicit project decision. \
                          Disabled when the server runs with --read-only.")]
    pub async fn vestige_record_decision(
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
