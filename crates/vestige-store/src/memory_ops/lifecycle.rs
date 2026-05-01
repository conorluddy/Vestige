//! Soft-delete lifecycle — `forget_memory` and `restore_memory`.
//!
//! Both flip `memories.status`; FTS sync is handled by triggers in migration
//! 0002 (`memory_after_soft_delete` drops FTS rows; `memory_after_restore`
//! re-inserts them). Neither path ever issues `DELETE FROM memories`.

use time::OffsetDateTime;

use vestige_core::MemoryId;

use crate::helpers::rfc3339;
use crate::{Result, Store};

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
}
