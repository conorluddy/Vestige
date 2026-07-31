//! Soft-delete lifecycle — `forget_memory` and `restore_memory` — plus
//! `revise_memory`, the in-place content-revision path (issue #130).
//!
//! `forget`/`restore` flip `memories.status`; FTS sync is handled by triggers
//! in migration 0002 (`memory_after_soft_delete` drops FTS rows;
//! `memory_after_restore` re-inserts them). `revise_memory` instead UPDATEs
//! the four `memory_representations` rows in place — `UNIQUE(memory_id,
//! representation_type)` means revision is an UPDATE, never an INSERT. Two
//! already-shipped triggers activate as a side effect of that UPDATE:
//! `memory_repr_after_update` (0002_fts.sql) resyncs FTS, and
//! `embedding_repr_content_changed` (0003_embeddings.sql) marks embeddings
//! stale on `content_hash` change. None of the three mutators in this file
//! ever issue `DELETE FROM memories`.

use rusqlite::OptionalExtension;
use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::memory::hash;
use vestige_core::representations::{depth_pick, derive};
use vestige_core::{MemoryId, RepresentationDepth};

use crate::helpers::rfc3339;
use crate::{Result, Store, StoreError};

/// Outcome of a successful [`Store::revise_memory`] call.
///
/// Carries only the `one_liner`s, not the full body — the caller already
/// has the new body (it supplied it), and the prior one-liner gives
/// `vestige revise --json` something worth echoing back without a second read.
#[derive(Debug, Clone)]
pub struct RevisionOutcome {
    /// The `one_liner` representation before this revision.
    pub prior_one_liner: String,
    /// The `one_liner` representation derived from the new body.
    pub new_one_liner: String,
}

impl Store {
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
        let updated = self.connection().execute(
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
        let updated = self.connection().execute(
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

    /// Revise a memory's content in place (`vestige revise`).
    ///
    /// UPDATEs all four `memory_representations` rows for `id` — never
    /// inserts, since `UNIQUE(memory_id, representation_type)` already holds
    /// exactly one row per depth. Also updates `memories.updated_at`.
    /// Preserves `id`, `created_at`, `recall_count`, `expand_count`, and
    /// `last_recalled_at` untouched — revision is a content edit, not a new
    /// memory and not a usage event. Appends a `memory.revised` event
    /// carrying the **full prior body** (not truncated) so the append-only
    /// `memory_events` journal stays a complete, reconstructable history.
    ///
    /// # Guards (checked in this order, inside the transaction)
    ///
    /// - `new_body` must be non-empty after trimming — [`StoreError::Validation`].
    /// - `id` must resolve to an existing row — [`StoreError::NotFound`].
    /// - that row must have `status = 'active'` — [`StoreError::Validation`],
    ///   since the caller can fix it by restoring first. A soft-deleted (or
    ///   otherwise non-active) memory must be restored before it can be
    ///   revised; revision never silently succeeds on one.
    ///
    /// # Side effects (all in one transaction — everything lands or nothing does)
    ///
    /// Updating `memory_representations.content_hash` fires
    /// `memory_repr_after_update` (migration 0002, resyncs `memory_fts`) and
    /// `embedding_repr_content_changed` (migration 0003, marks that
    /// representation's embeddings `stale`) automatically — no hand-rolled
    /// FTS or staleness logic lives here.
    pub fn revise_memory(&mut self, id: &MemoryId, new_body: &str) -> Result<RevisionOutcome> {
        let trimmed_body = new_body.trim();
        if trimmed_body.is_empty() {
            return Err(StoreError::Validation(
                "revised memory body must not be empty".into(),
            ));
        }

        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        let tx = self.connection_mut().transaction()?;

        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM memories WHERE id = ?1",
                rusqlite::params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        match status.as_deref() {
            None => {
                return Err(StoreError::NotFound(format!(
                    "no memory row for `{}`",
                    id.as_str()
                )))
            }
            Some("active") => {}
            Some(s) => {
                return Err(StoreError::Validation(format!(
                    "memory `{}` has status `{s}`, expected `active` — restore it before revising",
                    id.as_str()
                )))
            }
        }

        // Snapshot the current representations before any UPDATE touches
        // them — needed for the event payload and the returned outcome.
        let mut prior_one_liner = String::new();
        let mut prior_body = String::new();
        let mut prior_content_hash = String::new();
        {
            let mut stmt = tx.prepare(
                "SELECT representation_type, content, content_hash
                 FROM memory_representations
                 WHERE memory_id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![id.as_str()])?;
            while let Some(row) = rows.next()? {
                let depth_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                match depth_str.as_str() {
                    "one_liner" => prior_one_liner = content,
                    "full" => {
                        prior_body = content;
                        let content_hash: Option<String> = row.get(2)?;
                        prior_content_hash = content_hash.unwrap_or_default();
                    }
                    _ => {}
                }
            }
        }

        // Re-derive all four depths from the new body and UPDATE each
        // representation row in place (never INSERT — the row already
        // exists for every depth from the original `record_memory` call).
        let derived = derive(trimmed_body);
        let mut new_one_liner = String::new();
        let mut new_content_hash = String::new();
        for depth in [
            RepresentationDepth::OneLiner,
            RepresentationDepth::Summary,
            RepresentationDepth::Compressed,
            RepresentationDepth::Full,
        ] {
            let content = depth_pick(depth, &derived).to_string();
            let content_hash = hash(&content);
            tx.execute(
                "UPDATE memory_representations
                 SET content = ?1, content_hash = ?2, updated_at = ?3
                 WHERE memory_id = ?4 AND representation_type = ?5",
                rusqlite::params![content, content_hash, now_str, id.as_str(), depth.as_str()],
            )?;
            match depth {
                RepresentationDepth::OneLiner => new_one_liner = content,
                RepresentationDepth::Full => new_content_hash = content_hash,
                _ => {}
            }
        }

        // Narrow UPDATE — must never SET `status`. Every trigger on
        // `memories` is scoped `AFTER UPDATE OF status` (see the invariant
        // documented on `Store::bump_recall_stats` in fetch.rs); widening
        // this would risk spuriously re-firing soft-delete/restore FTS sync.
        tx.execute(
            "UPDATE memories SET updated_at = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), now_str],
        )?;

        let payload = serde_json::json!({
            "memory_id": id.as_str(),
            "prior_body": prior_body,
            "prior_one_liner": prior_one_liner,
            "prior_content_hash": prior_content_hash,
            "new_content_hash": new_content_hash,
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        let project_id: String = tx.query_row(
            "SELECT project_id FROM memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )?;
        // Populate memory_id directly (migration 0005) alongside payload_json,
        // same convention as append_status_event and record_memory.
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, memory_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                event_id,
                project_id,
                "memory.revised",
                payload,
                id.as_str(),
                now_str,
            ],
        )?;

        tx.commit()?;

        Ok(RevisionOutcome {
            prior_one_liner,
            new_one_liner,
        })
    }
}
