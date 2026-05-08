//! `vestige_propose_candidate` tool — proposes a memory candidate for the
//! assimilation inbox (PRD §10.3). Gated by `mcp.allow_propose_candidate`
//! and the global `read_only` flag.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_core::{MemoryType, NewCandidate, NewCandidateSource};
use vestige_engine::{error::EngineError, propose_candidate, ProposeOutcome};

use crate::server::{err, ok_json, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposeCandidateParams {
    /// Memory type: decision | observation | note | preference | question | summary
    pub r#type: String,
    #[serde(default)]
    pub title: Option<String>,
    pub body: String,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: f32,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub source: Option<ProposeSource>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposeSource {
    pub r#type: String,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

fn default_importance() -> f32 {
    0.5
}

fn default_confidence() -> f32 {
    0.8
}

// === OUTPUT SHAPE ===

#[derive(Debug, Serialize)]
struct ProposeCandidateResponse {
    candidate_id: String,
    status: String,
    similar_memories: Vec<SimilarMemoryJson>,
    similar_candidates: Vec<SimilarCandidateJson>,
}

#[derive(Debug, Serialize)]
struct SimilarMemoryJson {
    id: String,
    title: String,
    score: f32,
}

#[derive(Debug, Serialize)]
struct SimilarCandidateJson {
    id: String,
    title: String,
    score: f32,
}

impl From<ProposeOutcome> for ProposeCandidateResponse {
    fn from(outcome: ProposeOutcome) -> Self {
        Self {
            candidate_id: outcome.candidate_id.as_str().to_string(),
            status: outcome.status.as_str().to_string(),
            similar_memories: outcome
                .similar_memories
                .into_iter()
                .map(|m| SimilarMemoryJson {
                    id: m.id.as_str().to_string(),
                    title: m.title,
                    score: m.score,
                })
                .collect(),
            similar_candidates: outcome
                .similar_candidates
                .into_iter()
                .map(|c| SimilarCandidateJson {
                    id: c.id.as_str().to_string(),
                    title: c.title,
                    score: c.score,
                })
                .collect(),
        }
    }
}

// === TOOL ROUTER ===

#[tool_router(router = propose_candidate_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Propose a memory candidate for the assimilation inbox. \
                       Candidates are queued for human review before becoming durable memories. \
                       Returns dedup hints (similar_memories, similar_candidates) when found. \
                       Disabled when the server runs with --read-only or when \
                       mcp.allow_propose_candidate = false."
    )]
    pub async fn vestige_propose_candidate(
        &self,
        Parameters(p): Parameters<ProposeCandidateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;

        if inner.read_only {
            return Err(err(
                "READ_ONLY",
                "MCP server is read-only; vestige_propose_candidate is disabled",
                false,
            ));
        }

        if !inner.config.mcp.allow_propose_candidate {
            return Err(err(
                "CANDIDATE_DISABLED",
                "Candidate proposal is disabled in this project's MCP config.",
                false,
            ));
        }

        if p.body.trim().is_empty() {
            return Err(err("VALIDATION", "body must not be empty", false));
        }

        let proposed_type = MemoryType::from_str(&p.r#type).map_err(|_| {
            err(
                "INVALID_TYPE",
                format!("unknown memory type: {}", p.r#type),
                false,
            )
        })?;

        let source = p.source.map(|s| NewCandidateSource {
            source_type: s.r#type,
            source_ref: s.r#ref,
            source_content: s.content,
        });

        let project_id = inner.project_id.clone();
        let new_candidate = NewCandidate {
            project_id: project_id.clone(),
            proposed_type,
            body: p.body,
            rationale: p.rationale,
            title_override: p.title,
            importance: p.importance,
            confidence: p.confidence,
            source,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        };

        let outcome = propose_candidate(&mut inner.store, &project_id, new_candidate)
            .map_err(map_engine_error)?;

        ok_json(&ProposeCandidateResponse::from(outcome))
    }
}

// === PRIVATE HELPERS ===

fn map_engine_error(e: EngineError) -> ErrorData {
    match e {
        EngineError::CandidateNotFound { id } => err(
            "CANDIDATE_NOT_FOUND",
            format!("candidate not found: {id}"),
            false,
        ),
        EngineError::CandidateNotPending { status } => err(
            "CANDIDATE_NOT_PENDING",
            format!("candidate is not pending (status = {status})"),
            false,
        ),
        EngineError::OutOfScope => err(
            "OUT_OF_SCOPE",
            "candidate belongs to another project",
            false,
        ),
        EngineError::Validation { message } => err("VALIDATION", message, false),
        EngineError::Store(e) => err("STORE_FAILED", e.to_string(), true),
        EngineError::Embed(e) => err("EMBED_FAILED", e.to_string(), false),
        EngineError::EmbeddingsUnavailable(msg) => err("EMBEDDINGS_UNAVAILABLE", msg, false),
        EngineError::Core(e) => err("CORE_ERROR", e.to_string(), false),
        EngineError::TraceNotFound { .. } => err("TRACE_NOT_FOUND", e.to_string(), false),
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_importance_is_half() {
        assert!((default_importance() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn default_confidence_is_high() {
        assert!((default_confidence() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_type_from_str_rejects_garbage() {
        assert!(MemoryType::from_str("not_a_type").is_err());
    }

    #[test]
    fn memory_type_from_str_accepts_decision() {
        assert!(MemoryType::from_str("decision").is_ok());
    }
}
