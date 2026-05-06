//! `vestige_get_candidate` tool — fetches a single candidate at full fidelity
//! (PRD §10.3). Read-only; verifies project scope before returning.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::Deserialize;

use vestige_core::CandidateId;

use crate::server::{err, ok_json, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCandidateParams {
    pub candidate_id: String,
}

// === TOOL ROUTER ===

#[tool_router(router = get_candidate_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Fetch a single candidate at full fidelity: id, status, proposed_type, \
                       title, one_liner, summary, full_body, rationale, confidence, importance, \
                       sources, duplicate links, approval/rejection info, and timestamps. \
                       Verifies the candidate belongs to the current project."
    )]
    pub async fn vestige_get_candidate(
        &self,
        Parameters(p): Parameters<GetCandidateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        let id = CandidateId::new(&p.candidate_id).map_err(|e| {
            err(
                "INVALID_CANDIDATE_ID",
                format!("invalid candidate ID: {e}"),
                false,
            )
        })?;

        let candidate = inner
            .store
            .get_candidate(&id)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
            .ok_or_else(|| {
                err(
                    "CANDIDATE_NOT_FOUND",
                    format!("no candidate found with id: {}", id.as_str()),
                    false,
                )
            })?;

        if candidate.project_id != inner.project_id {
            return Err(err(
                "OUT_OF_SCOPE",
                "candidate belongs to another project",
                false,
            ));
        }

        ok_json(&candidate)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_id_new_rejects_wrong_prefix() {
        assert!(CandidateId::new("mem_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
    }

    #[test]
    fn candidate_id_new_rejects_empty() {
        assert!(CandidateId::new("").is_err());
    }

    #[test]
    fn candidate_id_new_accepts_cand_prefix() {
        assert!(CandidateId::new("cand_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
    }
}
