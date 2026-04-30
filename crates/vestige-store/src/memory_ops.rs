//! `Store` methods for memory CRUD, FTS search, soft-delete, and event logging.
//!
//! # Soft-delete invariant
//!
//! **No `DELETE FROM memories` anywhere in this file.** Every lifecycle
//! transition is a status flip (`active` → `deleted` or back). The FTS index
//! stays consistent through SQLite triggers defined in migration 0002:
//! soft-delete fires `memory_after_soft_delete` (drops FTS rows) and restore
//! fires `memory_after_restore` (re-inserts them).
//!
//! # Event journal
//!
//! Every mutating operation appends a row to `memory_events`. The journal is
//! append-only — no event row is ever updated or deleted. It is the canonical
//! audit trail and can reconstruct `memories` if that table were wiped.
//!
//! # FTS search strategy
//!
//! `search_memories` runs the FTS5 `MATCH` query in isolation (no JOIN) to
//! avoid bm25 aggregation limitations in some SQLite builds. Project-scope and
//! status filtering are applied client-side in Rust after the FTS pass, which
//! is acceptable because project DBs are already per-project (PRD §9).

use std::str::FromStr;

use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::{
    FetchedMemory, ListFilter, Memory, MemoryBundle, MemoryCounts, MemoryId, MemoryStatus,
    MemoryType, ProjectId, RepresentationDepth, RepresentationRow, SearchFilter, SearchHit,
    SourceRow,
};

use crate::helpers::{invalid_id_to_sqlite, parse_rfc3339, rfc3339};
use crate::{Result, Store, StoreError};

// === IMPL STORE ===

impl Store {
    /// Count memories grouped by status for this project.
    ///
    /// Returns a [`MemoryCounts`] with `active` and `deleted` tallies.
    /// Used by `vestige status` to display the project summary.
    pub fn memory_counts(&self, project_id: &ProjectId) -> Result<MemoryCounts> {
        let mut stmt = self.conn.prepare(
            "SELECT status, COUNT(*) FROM memories WHERE project_id = ?1 GROUP BY status",
        )?;
        let mut active = 0i64;
        let mut deleted = 0i64;
        let mut rows = stmt.query(rusqlite::params![project_id.as_str()])?;
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            match status.as_str() {
                "active" => active = count,
                "deleted" => deleted = count,
                _ => {}
            }
        }
        Ok(MemoryCounts { active, deleted })
    }

    /// Append a structured event to the `memory_events` journal.
    ///
    /// The journal is append-only — this is the only write path. `event_type`
    /// should follow dot-namespaced convention (`"memory.recorded"`,
    /// `"memory.forgotten"`, etc.). `payload_json` is optional free-form JSON.
    ///
    /// Side-effect: inserts one row into `memory_events`.
    pub fn record_event(
        &self,
        project_id: &ProjectId,
        event_type: &str,
        payload_json: Option<&str>,
    ) -> Result<()> {
        let id = format!("evt_{}", Ulid::new());
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        self.conn.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, project_id.as_str(), event_type, payload_json, now_str],
        )?;
        Ok(())
    }

    /// Persist a [`MemoryBundle`] and record a `memory.recorded` journal event.
    ///
    /// **Atomicity** — everything runs inside a single `BEGIN … COMMIT`
    /// transaction: the `memories` row, all `memory_representations` rows
    /// (typically four: handle / one-liner / summary / compressed), the
    /// optional `memory_sources` row, and the `memory_events` entry. Either
    /// all rows land or none do.
    ///
    /// **FTS** — `memory_representations` INSERT triggers (migration 0002)
    /// automatically populate `memory_fts` within the same transaction.
    ///
    /// Side-effects: inserts into `memories`, `memory_representations`,
    /// optionally `memory_sources`, and `memory_events`.
    pub fn record_memory(&mut self, bundle: &MemoryBundle) -> Result<()> {
        let tx = self.conn.transaction()?;
        let m = &bundle.memory;
        let created_str = rfc3339(m.created_at)?;
        let updated_str = rfc3339(m.updated_at)?;

        tx.execute(
            "INSERT INTO memories (id, project_id, type, status, confidence, importance, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                m.id.as_str(),
                m.project_id.as_str(),
                m.r#type.as_str(),
                m.status.as_str(),
                m.confidence,
                m.importance,
                created_str,
                updated_str,
            ],
        )?;

        for rep in &bundle.representations {
            let id = format!("rep_{}", Ulid::new());
            tx.execute(
                "INSERT INTO memory_representations
                    (id, memory_id, representation_type, content, token_count, content_hash, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)",
                rusqlite::params![
                    id,
                    rep.memory_id.as_str(),
                    rep.depth.as_str(),
                    rep.content,
                    rep.content_hash,
                    created_str,
                ],
            )?;
        }

        if let Some(src) = &bundle.source {
            let id = format!("src_{}", Ulid::new());
            tx.execute(
                "INSERT INTO memory_sources
                    (id, memory_id, source_type, source_ref, source_content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    src.memory_id.as_str(),
                    src.source_type,
                    src.source_ref,
                    src.source_content,
                    created_str,
                ],
            )?;
        }

        let payload = serde_json::json!({
            "memory_id": m.id.as_str(),
            "type": m.r#type.as_str(),
            "importance": m.importance,
            "has_source": bundle.source.is_some(),
            "source_truncated": bundle.source.as_ref().map(|s| s.truncated).unwrap_or(false),
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                event_id,
                m.project_id.as_str(),
                "memory.recorded",
                payload,
                created_str,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Fetch a memory by ID, joining all representations and sources.
    ///
    /// Returns `None` if no row with that ID exists (any status). Callers that
    /// need active-only should check `FetchedMemory::memory.status` afterward.
    pub fn get_memory(&self, id: &MemoryId) -> Result<Option<FetchedMemory>> {
        let memory = match self.fetch_memory_row(id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        let representations = self.fetch_representations(id)?;
        let sources = self.fetch_sources(id)?;
        Ok(Some(FetchedMemory {
            memory,
            representations,
            sources,
        }))
    }

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

        let mut stmt = self.conn.prepare(&sql)?;
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

    /// FTS5-backed search over the project's active memories. Returns the
    /// best-matching representation's bm25 score per memory; lower bm25 is a
    /// better match. Composite ranking (importance / type / recency) is
    /// applied by `vestige-core` so the rules stay in pure code.
    pub fn search_memories(
        &self,
        project_id: &ProjectId,
        fts_query: &str,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchHit>> {
        if fts_query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // bm25() can only be used in queries that directly MATCH the FTS
        // table without intervening JOINs/CTEs in some SQLite builds. We
        // run the FTS pass in isolation and apply project/status/type
        // filters client-side. Project DBs are per-project (PRD §9), so the
        // candidate set is already scoped.
        // bm25() can only be called once per row and not inside aggregates
        // in some SQLite builds. Pull raw row scores, dedupe + filter in
        // Rust. Project DBs are per-project (PRD §9), so scoping is local.
        let candidate_limit = filter
            .limit
            .map(|n| n.saturating_mul(8).max(50))
            .unwrap_or(200);
        let mut stmt = self.conn.prepare(
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
        use std::collections::HashMap;
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

    /// Soft-delete a memory (`vestige forget`).
    ///
    /// Flips `status` from `'active'` to `'deleted'` and sets `deleted_at`.
    /// The `memory_after_soft_delete` trigger (migration 0002) synchronously
    /// removes the memory's rows from `memory_fts`, so it immediately drops
    /// out of search results. A `memory.forgotten` event is appended to the
    /// journal. No row is ever hard-deleted.
    ///
    /// Returns `true` if the row existed in `active` state and was updated;
    /// `false` if not found or already deleted (idempotent, not an error).
    pub fn forget_memory(&mut self, id: &MemoryId) -> Result<bool> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        let updated = self.conn.execute(
            "UPDATE memories
             SET status = 'deleted', deleted_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status = 'active'",
            rusqlite::params![id.as_str(), now_str],
        )?;
        if updated > 0 {
            self.append_status_event(id, "memory.forgotten", &now_str)?;
        }
        Ok(updated > 0)
    }

    /// Restore a soft-deleted memory (`vestige restore`).
    ///
    /// Flips `status` from `'deleted'` back to `'active'` and clears
    /// `deleted_at`. The `memory_after_restore` trigger (migration 0002)
    /// synchronously re-inserts the memory's representations into `memory_fts`,
    /// making it searchable again. A `memory.restored` event is appended.
    ///
    /// Note: embeddings are left stale after restore (PRD §8.4) — they will
    /// re-embed on the next `vestige embed` run.
    ///
    /// Returns `true` if the row existed in `deleted` state and was updated.
    pub fn restore_memory(&mut self, id: &MemoryId) -> Result<bool> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        let updated = self.conn.execute(
            "UPDATE memories
             SET status = 'active', deleted_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status = 'deleted'",
            rusqlite::params![id.as_str(), now_str],
        )?;
        if updated > 0 {
            self.append_status_event(id, "memory.restored", &now_str)?;
        }
        Ok(updated > 0)
    }

    /// Append a status-transition event for `id` to the `memory_events` journal.
    ///
    /// Looks up `project_id` from the `memories` row, then inserts one
    /// `memory_events` row with `{ "memory_id": "…" }` as the payload.
    /// Called by `forget_memory` and `restore_memory`; not for direct use.
    pub(crate) fn append_status_event(
        &self,
        id: &MemoryId,
        event_type: &str,
        when: &str,
    ) -> Result<()> {
        // Look up project_id for the event payload.
        let project_id: String = self.conn.query_row(
            "SELECT project_id FROM memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )?;
        let payload = serde_json::json!({ "memory_id": id.as_str() }).to_string();
        let event_id = format!("evt_{}", Ulid::new());
        self.conn.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![event_id, project_id, event_type, payload, when],
        )?;
        Ok(())
    }

    /// Fetch the raw `memories` row for `id`. No representations or sources.
    pub(crate) fn fetch_memory_row(&self, id: &MemoryId) -> Result<Option<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, type, status, confidence, importance,
                    created_at, updated_at, deleted_at
             FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_memory(row)?))
        } else {
            Ok(None)
        }
    }

    /// Fetch all `memory_representations` rows for `id`, ordered by type.
    pub(crate) fn fetch_representations(&self, id: &MemoryId) -> Result<Vec<RepresentationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id, representation_type, content, content_hash
             FROM memory_representations
             WHERE memory_id = ?1
             ORDER BY representation_type",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![id.as_str()], |row| {
                let mid: String = row.get(0)?;
                let depth_str: String = row.get(1)?;
                let content: String = row.get(2)?;
                let content_hash: Option<String> = row.get(3)?;
                Ok((mid, depth_str, content, content_hash))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (mid, depth_str, content, content_hash) in rows {
            let memory_id = MemoryId::from_str(&mid).map_err(invalid_id_to_sqlite)?;
            let depth = RepresentationDepth::from_str(&depth_str).map_err(invalid_id_to_sqlite)?;
            out.push(RepresentationRow {
                memory_id,
                depth,
                content,
                content_hash: content_hash.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Fetch all `memory_sources` rows for `id`, ordered by `created_at`.
    ///
    /// The `truncated` field is always `false` on retrieval — truncation is
    /// a build-time concern applied before storage, not persisted as metadata.
    pub(crate) fn fetch_sources(&self, id: &MemoryId) -> Result<Vec<SourceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id, source_type, source_ref, source_content
             FROM memory_sources
             WHERE memory_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![id.as_str()], |row| {
                let mid: String = row.get(0)?;
                let st: String = row.get(1)?;
                let sr: Option<String> = row.get(2)?;
                let sc: Option<String> = row.get(3)?;
                Ok((mid, st, sr, sc))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (mid, st, sr, sc) in rows {
            let memory_id = MemoryId::from_str(&mid).map_err(invalid_id_to_sqlite)?;
            out.push(SourceRow {
                memory_id,
                source_type: st,
                source_ref: sr,
                source_content: sc,
                truncated: false, // truncation is a build-time concern; not persisted
            });
        }
        Ok(out)
    }
}

/// Map a `memories` SELECT row (columns 0–8) into a [`Memory`].
///
/// Column order must match the SELECT list in every caller:
/// `id, project_id, type, status, confidence, importance, created_at, updated_at, deleted_at`.
fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let id_str: String = row.get(0)?;
    let project_str: String = row.get(1)?;
    let type_str: String = row.get(2)?;
    let status_str: String = row.get(3)?;
    let confidence: f64 = row.get(4)?;
    let importance: f64 = row.get(5)?;
    let created_str: String = row.get(6)?;
    let updated_str: String = row.get(7)?;
    let deleted_str: Option<String> = row.get(8)?;

    let id = MemoryId::from_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let project_id = ProjectId::from_str(&project_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let r#type = MemoryType::from_str(&type_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status = MemoryStatus::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let created_at = parse_rfc3339(&created_str, 6).map_err(|e| match e {
        StoreError::Sqlite(err) => err,
        other => rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(other),
        ),
    })?;
    let updated_at = parse_rfc3339(&updated_str, 7).map_err(|e| match e {
        StoreError::Sqlite(err) => err,
        other => rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(other),
        ),
    })?;
    let deleted_at = match deleted_str {
        Some(s) => Some(parse_rfc3339(&s, 8).map_err(|e| match e {
            StoreError::Sqlite(err) => err,
            other => rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(other),
            ),
        })?),
        None => None,
    };

    Ok(Memory {
        id,
        project_id,
        r#type,
        status,
        confidence,
        importance,
        created_at,
        updated_at,
        deleted_at,
    })
}
