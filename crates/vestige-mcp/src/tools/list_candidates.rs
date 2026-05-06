//! `vestige_list_candidates` tool — lists candidates in the assimilation inbox
//! for the current project (PRD §10.3, §15.1). Read-only; no capability gate.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_core::{Candidate, CandidateStatus, MemoryType};
use vestige_store::CandidateFilter;

use crate::server::{err, ok_json, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListCandidatesParams {
    /// Lifecycle status filter: pending | approved | rejected | superseded.
    /// Defaults to "pending".
    #[serde(default)]
    pub status: Option<String>,
    /// Optional memory type filter: decision | observation | note | preference | question | summary.
    #[serde(default)]
    pub r#type: Option<String>,
    /// Maximum rows returned. Default: 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    /// When true, return both pending and rejected rows (overrides status filter).
    #[serde(default)]
    pub include_rejected: bool,
}

fn default_list_limit() -> u32 {
    50
}

// === OUTPUT SHAPE ===

#[derive(Debug, Serialize)]
struct ListCandidatesResponse {
    candidates: Vec<CandidateListItem>,
}

/// Compact projection of a candidate for list views (PRD §15.1).
/// `similar_memories` is empty in V0.2 — populated by future similarity index.
#[derive(Debug, Serialize)]
struct CandidateListItem {
    id: String,
    r#type: String,
    status: String,
    title: String,
    one_liner: String,
    confidence: f32,
    importance: f32,
    similar_memories: Vec<()>,
    created_at: String,
}

impl From<Candidate> for CandidateListItem {
    fn from(c: Candidate) -> Self {
        Self {
            id: c.id.as_str().to_string(),
            r#type: c.proposed_type.as_str().to_string(),
            status: c.status.as_str().to_string(),
            title: c.title,
            one_liner: c.one_liner,
            confidence: c.confidence,
            importance: c.importance,
            similar_memories: vec![],
            created_at: c.created_at.to_string(),
        }
    }
}

// === TOOL ROUTER ===

#[tool_router(router = list_candidates_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "List candidates in the assimilation inbox for the current project. \
                       Returns compact cards with id, type, status, title, one_liner, \
                       confidence, and importance. Use vestige_get_candidate for full detail."
    )]
    pub async fn vestige_list_candidates(
        &self,
        Parameters(p): Parameters<ListCandidatesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        let status = match &p.status {
            Some(s) => {
                let parsed = CandidateStatus::from_str(s)
                    .map_err(|_| err("INVALID_STATUS", format!("unknown status: {s}"), false))?;
                Some(parsed)
            }
            None => Some(CandidateStatus::Pending),
        };

        let proposed_type = match &p.r#type {
            Some(t) => {
                let parsed = MemoryType::from_str(t)
                    .map_err(|_| err("INVALID_TYPE", format!("unknown memory type: {t}"), false))?;
                Some(parsed)
            }
            None => None,
        };

        let filter = CandidateFilter {
            status,
            proposed_type,
            limit: Some(p.limit),
            include_rejected: p.include_rejected,
        };

        let candidates = inner
            .store
            .list_candidates(&inner.project_id, &filter)
            .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;

        let response = ListCandidatesResponse {
            candidates: candidates
                .into_iter()
                .map(CandidateListItem::from)
                .collect(),
        };

        ok_json(&response)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_fifty() {
        assert_eq!(default_list_limit(), 50);
    }

    #[test]
    fn candidate_status_from_str_rejects_garbage() {
        assert!(CandidateStatus::from_str("not_a_status").is_err());
    }

    #[test]
    fn candidate_status_from_str_accepts_pending() {
        assert!(CandidateStatus::from_str("pending").is_ok());
    }

    #[test]
    fn memory_type_from_str_rejects_garbage() {
        assert!(MemoryType::from_str("bogus").is_err());
    }
}
