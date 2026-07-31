//! `vestige_revise_memory` tool — revises a memory's content in place.
//! Gated by `mcp.allow_revise` and the global `read_only` flag. Delegates to
//! `Store::revise_memory` (issue #130); this file is a thin adapter — no
//! business logic, per CLAUDE.md's MCP intent-not-mechanics rule.

use std::str::FromStr;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_core::MemoryId;
use vestige_store::{RevisionOutcome, StoreError};

use crate::server::{err, ok_json, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviseMemoryParams {
    /// The `mem_<ULID>` handle of the memory to revise.
    pub id: String,
    /// The full replacement body. All four representation depths are
    /// re-derived from it; must be non-empty after trimming.
    pub body: String,
}

// === OUTPUT SHAPE ===

#[derive(Debug, Serialize)]
struct ReviseMemoryResponse {
    memory_id: String,
    prior_one_liner: String,
    new_one_liner: String,
}

// === TOOL ROUTER ===

#[tool_router(router = revise_memory_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Revise a memory's content in place. Re-derives all representation \
                       depths from the new body and returns the prior and new one-liners. \
                       The memory must be active — restore a soft-deleted memory before \
                       revising it. Disabled when the server runs with --read-only or when \
                       mcp.allow_revise = false."
    )]
    pub async fn vestige_revise_memory(
        &self,
        Parameters(p): Parameters<ReviseMemoryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut inner = self.inner.lock().await;

        if inner.read_only {
            return Err(err(
                "READ_ONLY",
                "MCP server is read-only; vestige_revise_memory is disabled",
                false,
            ));
        }

        if !inner.config.mcp.allow_revise {
            return Err(err(
                "REVISE_DISABLED",
                "Memory revision is disabled in this project's MCP config.",
                false,
            ));
        }

        let id = MemoryId::from_str(&p.id).map_err(|e| err("INVALID_ID", e.to_string(), false))?;

        // Scope check before mutating — same pattern as vestige_expand /
        // vestige_get_candidate: a memory belonging to another project must
        // never be readable or writable from here.
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

        let outcome = inner
            .store
            .revise_memory(&id, &p.body)
            .map_err(map_store_error)?;

        ok_json(&ReviseMemoryResponse::from((id, outcome)))
    }
}

// === PRIVATE HELPERS ===

impl From<(MemoryId, RevisionOutcome)> for ReviseMemoryResponse {
    fn from((id, outcome): (MemoryId, RevisionOutcome)) -> Self {
        Self {
            memory_id: id.as_str().to_string(),
            prior_one_liner: outcome.prior_one_liner,
            new_one_liner: outcome.new_one_liner,
        }
    }
}

/// Maps `StoreError` by variant, never by string-sniffing the message — the
/// distinction between `NotFound` and `Validation` is exactly what issue #130
/// added those variants for.
fn map_store_error(e: StoreError) -> ErrorData {
    match e {
        StoreError::NotFound(msg) => err("MEMORY_NOT_FOUND", msg, false),
        StoreError::Validation(msg) => err("VALIDATION", msg, false),
        other => err("STORE_FAILED", other.to_string(), true),
    }
}
