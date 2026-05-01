//! `Store` impl methods scoped to embedding-job tracking and per-representation
//! lookup. These run on `&self` / `&mut self` so they live on the type, unlike
//! the free `pub(crate) fn` helpers in the sibling modules.

use ulid::Ulid;

use vestige_core::{MemoryId, RepresentationDepth};

use crate::{Result, Store};

use super::rfc3339_now;

impl Store {
    /// Resolve the `memory_representations.id` for a given `(memory_id, depth)` pair.
    ///
    /// `vestige-engine` calls this to look up the opaque representation row ID
    /// before calling [`Store::record_embedding`] or checking
    /// [`Store::has_active_embedding`]. Keeps raw SQL out of the engine layer.
    ///
    /// Returns `Some(id)` when a row for that depth exists, `None` otherwise
    /// (e.g. a memory that only has `handle` and `one_liner` so far).
    pub fn repr_id_for_depth(
        &self,
        memory_id: &MemoryId,
        depth: RepresentationDepth,
    ) -> Result<Option<String>> {
        let mut stmt = self.connection().prepare(
            "SELECT id FROM memory_representations
             WHERE memory_id = ?1 AND representation_type = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![memory_id.as_str(), depth.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Return `true` if an active embedding exists for `(repr_id, provider, model)`.
    ///
    /// Used by `vestige-engine` to skip re-embedding representations whose
    /// vector is still current. The guard prevents redundant model calls when
    /// `vestige embed --all` is re-run on an already-embedded project.
    ///
    /// Only `status = 'active'` rows count; stale embeddings return `false`
    /// so the engine treats them as missing and re-queues them.
    pub fn has_active_embedding(&self, repr_id: &str, provider: &str, model: &str) -> Result<bool> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM memory_embeddings
             WHERE representation_id = ?1
               AND provider = ?2
               AND model = ?3
               AND status = 'active'",
            rusqlite::params![repr_id, provider, model],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Record a failed embedding attempt in the `embedding_jobs` table.
    ///
    /// `vestige-engine` calls this when an embedding provider returns an error
    /// so the failure is visible via `vestige embeddings status` without
    /// crashing the overall embed run. The row is inserted with `status =
    /// 'failed'`; the `error` column carries the provider's error message.
    ///
    /// This does **not** affect the `memory_embeddings` table — the
    /// representation is simply left without an active embedding and will
    /// appear in the `missing_embeddings` count until a future run succeeds.
    pub fn record_failed_embedding_job(
        &mut self,
        memory_id: &MemoryId,
        repr_id: &str,
        depth: RepresentationDepth,
        provider: &str,
        model: &str,
        error: &str,
    ) -> Result<()> {
        let job_id = format!("job_{}", Ulid::new());
        let now_str = rfc3339_now()?;
        self.connection_mut().execute(
            "INSERT INTO embedding_jobs
                (id, memory_id, representation_id, representation_type,
                 provider, model, status, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'failed', ?7, ?8, ?8)",
            rusqlite::params![
                job_id,
                memory_id.as_str(),
                repr_id,
                depth.as_str(),
                provider,
                model,
                error,
                now_str,
            ],
        )?;
        Ok(())
    }
}
