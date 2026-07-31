//! Per-memory reads: `get_memory`, `memory_counts`, and the
//! representation / source helpers used by every list/search path.
//!
//! Also owns the usage-counter bump methods (`bump_recall_stats`,
//! `bump_expand_stats`, issue #128) — grouped here with the other
//! single-memory operations rather than in `record.rs`, since they update an
//! existing row instead of inserting one.

use std::str::FromStr;

use time::OffsetDateTime;

use vestige_core::{
    FetchedMemory, Memory, MemoryCounts, MemoryId, ProjectId, RepresentationDepth,
    RepresentationRow, SourceRow,
};

use crate::helpers::{invalid_id_to_sqlite, rfc3339};
use crate::{Result, Store};

use super::row_to_memory;

impl Store {
    /// Count memories grouped by status for this project.
    ///
    /// Returns a [`MemoryCounts`] with `active` and `deleted` tallies.
    /// Used by `vestige status` to display the project summary.
    pub fn memory_counts(&self, project_id: &ProjectId) -> Result<MemoryCounts> {
        let mut stmt = self.connection().prepare(
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

    /// RFC-3339 timestamp of the most recently created active memory in this
    /// project, or `None` if the project has no active memories. Used by the
    /// daemon to surface a "last activity" signal in `daemon.status.json`.
    pub fn latest_active_memory_at(&self, project_id: &ProjectId) -> Result<Option<String>> {
        let mut stmt = self.connection().prepare(
            "SELECT MAX(created_at) FROM memories
             WHERE project_id = ?1 AND status = 'active'",
        )?;
        let mut rows = stmt.query(rusqlite::params![project_id.as_str()])?;
        if let Some(row) = rows.next()? {
            let ts: Option<String> = row.get(0)?;
            Ok(ts)
        } else {
            Ok(None)
        }
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

    /// Record that `ids` were returned by a search: increment `recall_count`
    /// and stamp `last_recalled_at`. Batched into one transaction; an empty
    /// slice is a no-op.
    ///
    /// # Trigger-safety invariant
    ///
    /// **This UPDATE must never SET `status`** — not even redundantly to its
    /// current value. Every trigger on `memories` is scoped
    /// `AFTER UPDATE OF status`: `memory_after_soft_delete` and
    /// `memory_after_restore` (`0002_fts.sql:31,39`) and
    /// `embedding_memory_soft_deleted` (`0003_embeddings.sql:74`). A
    /// counter-only UPDATE therefore cannot fire them. Widening this into a
    /// full-row update would silently re-run FTS sync and mark embeddings
    /// stale on every search. Do not "tidy" it into one.
    ///
    /// Takes `&self` rather than `&mut self` so read paths holding a shared
    /// `&Store` can bump without threading a mutable borrow through every
    /// caller. Same posture as [`Store::record_query_event`].
    pub fn bump_recall_stats(&self, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = rfc3339(OffsetDateTime::now_utc())?;
        let tx = self.connection().unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE memories
                 SET recall_count = recall_count + 1, last_recalled_at = ?2
                 WHERE id = ?1",
            )?;
            for id in ids {
                stmt.execute(rusqlite::params![id.as_str(), now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record that `id` was expanded to a deeper representation: increment
    /// `expand_count` only, leaving `recall_count` and `last_recalled_at`
    /// untouched. Expansion is a distinct signal from recall — a memory can be
    /// surfaced often but rarely read in full.
    ///
    /// Carries the same trigger-safety invariant as
    /// [`Store::bump_recall_stats`]: never SET `status` here.
    pub fn bump_expand_stats(&self, id: &MemoryId) -> Result<()> {
        self.connection().execute(
            "UPDATE memories SET expand_count = expand_count + 1 WHERE id = ?1",
            rusqlite::params![id.as_str()],
        )?;
        Ok(())
    }

    /// Fetch the raw `memories` row for `id`. No representations or sources.
    pub(crate) fn fetch_memory_row(&self, id: &MemoryId) -> Result<Option<Memory>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, project_id, type, status, confidence, importance,
                    created_at, updated_at, deleted_at,
                    recall_count, expand_count, last_recalled_at, superseded_by
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
        let mut stmt = self.connection().prepare(
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
        let mut stmt = self.connection().prepare(
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
