//! Explicit project-scoped deletion from a human review session.

use serde::Serialize;
use vestige_core::{MemoryId, MemoryStatus, ProjectId};
use vestige_store::Store;

use crate::error::Result;

/// Per-ID outcome. An explicitly selected memory need not match the queue's
/// age/importance heuristic, but it must belong to the current project.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewForgetStatus {
    Deleted,
    AlreadyDeleted,
    NotFound,
    OutOfScope,
}

impl ReviewForgetStatus {
    /// Stable label for CLI output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::AlreadyDeleted => "already_deleted",
            Self::NotFound => "not_found",
            Self::OutOfScope => "out_of_scope",
        }
    }
}

/// Soft-delete one explicitly selected memory without crossing project scope.
pub fn forget_review_memory(
    store: &mut Store,
    project_id: &ProjectId,
    id: &MemoryId,
) -> Result<ReviewForgetStatus> {
    let Some(fetched) = store.get_memory(id)? else {
        return Ok(ReviewForgetStatus::NotFound);
    };
    if &fetched.memory.project_id != project_id {
        return Ok(ReviewForgetStatus::OutOfScope);
    }
    if fetched.memory.status == MemoryStatus::Deleted {
        return Ok(ReviewForgetStatus::AlreadyDeleted);
    }
    if store.forget_memory(id)? {
        Ok(ReviewForgetStatus::Deleted)
    } else {
        // Another CLI may have forgotten it after the read above.
        Ok(ReviewForgetStatus::AlreadyDeleted)
    }
}
