//! Read-only hygiene queue: old, unused, low-importance memories.

use vestige_core::{FetchedMemory, Memory, ProjectId};

use super::row_to_memory;
use crate::{Result, Store};

impl Store {
    /// List active memories eligible for human review, oldest first.
    ///
    /// Decisions and project summaries are protected. Both usage counters
    /// must be zero; age and importance comparisons are strictly exclusive.
    /// Listing neither bumps counters nor writes query traces.
    pub fn list_review_candidates(
        &self,
        project_id: &ProjectId,
        min_age_days: u32,
        importance_ceiling: f64,
    ) -> Result<Vec<FetchedMemory>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, project_id, type, status, confidence, importance,
                    created_at, updated_at, deleted_at,
                    recall_count, expand_count, last_recalled_at, superseded_by
             FROM memories
             WHERE project_id = ?1 AND status = 'active'
               AND recall_count = 0 AND expand_count = 0
               AND type NOT IN ('decision', 'project_summary')
               AND importance < ?2
               AND julianday('now') - julianday(created_at) > ?3
             ORDER BY created_at ASC, id ASC",
        )?;
        let memories: Vec<Memory> = stmt
            .query_map(
                rusqlite::params![project_id.as_str(), importance_ceiling, min_age_days],
                row_to_memory,
            )?
            .collect::<std::result::Result<_, _>>()?;

        memories
            .into_iter()
            .map(|memory| {
                Ok(FetchedMemory {
                    representations: self.fetch_representations(&memory.id)?,
                    sources: self.fetch_sources(&memory.id)?,
                    memory,
                })
            })
            .collect()
    }
}
