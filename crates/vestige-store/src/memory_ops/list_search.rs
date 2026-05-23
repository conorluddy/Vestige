//! Bulk reads — `list_memories`, `recent_memories_by_created_at`, and FTS5 `search_memories`.

use std::collections::HashMap;
use std::str::FromStr;

use vestige_core::{
    FetchedMemory, ListFilter, Memory, MemoryId, MemoryStatus, ProjectId, SearchFilter, SearchHit,
};

use crate::helpers::invalid_id_to_sqlite;
use crate::{Result, Store};

use super::row_to_memory;

impl Store {
    /// List memories for a project, optionally filtered by type or status.
    ///
    /// Excludes deleted memories by default; set `filter.include_deleted` to
    /// include them. Results are ordered by `updated_at DESC`. Each returned
    /// [`FetchedMemory`] includes all representations and sources via N+1
    /// queries — appropriate for list sizes in the tens; not for bulk export.
    pub fn list_memories(
        &self,
        project_id: &ProjectId,
        filter: &ListFilter,
    ) -> Result<Vec<FetchedMemory>> {
        let mut sql = String::from(
            "SELECT id, project_id, type, status, confidence, importance,
                    created_at, updated_at, deleted_at
             FROM memories
             WHERE project_id = ?1",
        );
        if !filter.include_deleted {
            sql.push_str(" AND status = 'active'");
        }
        if filter.r#type.is_some() {
            sql.push_str(" AND type = ?2");
        }
        sql.push_str(" ORDER BY datetime(updated_at) DESC");
        if let Some(n) = filter.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }

        let mut stmt = self.connection().prepare(&sql)?;
        let memories: Vec<Memory> = match &filter.r#type {
            Some(t) => stmt
                .query_map(
                    rusqlite::params![project_id.as_str(), t.as_str()],
                    row_to_memory,
                )?
                .collect::<std::result::Result<_, _>>()?,
            None => stmt
                .query_map(rusqlite::params![project_id.as_str()], row_to_memory)?
                .collect::<std::result::Result<_, _>>()?,
        };

        let mut out = Vec::with_capacity(memories.len());
        for memory in memories {
            let representations = self.fetch_representations(&memory.id)?;
            let sources = self.fetch_sources(&memory.id)?;
            out.push(FetchedMemory {
                memory,
                representations,
                sources,
            });
        }
        Ok(out)
    }

    /// Active memories for a project, newest first, capped at `limit` rows.
    ///
    /// Excludes soft-deleted memories. Does not load representations or sources —
    /// callers that need full `MemoryCard` projections should follow up with
    /// `get_memory` on demand.
    pub fn recent_memories_by_created_at(
        &self,
        project_id: &ProjectId,
        limit: u32,
    ) -> Result<Vec<Memory>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, project_id, type, status, confidence, importance,
                    created_at, updated_at, deleted_at
             FROM memories
             WHERE project_id = ?1 AND status = 'active'
             ORDER BY datetime(created_at) DESC
             LIMIT ?2",
        )?;
        let memories: Vec<Memory> = stmt
            .query_map(
                rusqlite::params![project_id.as_str(), limit as i64],
                row_to_memory,
            )?
            .collect::<std::result::Result<_, _>>()?;
        Ok(memories)
    }

    /// FTS5-backed search over the project's active memories. Returns the
    /// best-matching representation's bm25 score per memory; lower bm25 is a
    /// better match. Composite ranking (importance / type / recency) is
    /// applied by `vestige-core` so the rules stay in pure code.
    ///
    /// `bm25()` can only be called once per row and not inside aggregates in
    /// some SQLite builds. We pull raw row scores, dedupe + filter in Rust.
    /// Project DBs are per-project (PRD §9), so scoping is local.
    pub fn search_memories(
        &self,
        project_id: &ProjectId,
        fts_query: &str,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchHit>> {
        if fts_query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let candidate_limit = filter
            .limit
            .map(|n| n.saturating_mul(8).max(50))
            .unwrap_or(200);
        let mut stmt = self.connection().prepare(
            "SELECT memory_id, bm25(memory_fts) AS score
             FROM memory_fts
             WHERE memory_fts MATCH ?1
             ORDER BY score ASC
             LIMIT ?2",
        )?;
        let raw: Vec<(String, f64)> = stmt
            .query_map(
                rusqlite::params![fts_query, candidate_limit as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
            )?
            .collect::<std::result::Result<_, _>>()?;

        // Best (lowest bm25) per memory_id, preserving sort order.
        let mut best: HashMap<String, f64> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for (id, score) in raw {
            match best.get(&id) {
                Some(prev) if *prev <= score => {}
                _ => {
                    if !best.contains_key(&id) {
                        order.push(id.clone());
                    }
                    best.insert(id, score);
                }
            }
        }

        let mut hits = Vec::new();
        for id_str in order {
            let bm25 = best[&id_str];
            let id = MemoryId::from_str(&id_str).map_err(invalid_id_to_sqlite)?;
            let fetched = match self.get_memory(&id)? {
                Some(f) => f,
                None => continue,
            };
            if fetched.memory.project_id != *project_id
                || fetched.memory.status != MemoryStatus::Active
            {
                continue;
            }
            if let Some(t) = &filter.r#type {
                if fetched.memory.r#type != *t {
                    continue;
                }
            }
            hits.push(SearchHit { fetched, bm25 });
            if let Some(limit) = filter.limit {
                if hits.len() as u32 >= limit {
                    break;
                }
            }
        }
        Ok(hits)
    }
}
