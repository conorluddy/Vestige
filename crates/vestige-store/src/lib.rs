//! SQLite-backed persistence for Vestige (PRD §9).
//!
//! Owns connection management, the migration runner, and FTS5 sync triggers.
//! Higher-level business logic lives in `vestige-core`; `vestige-store` is a
//! pure persistence adapter that never makes domain decisions.
//!
//! # Key invariants
//!
//! - **WAL mode** — every `Store::open` sets `journal_mode = WAL` and
//!   `foreign_keys = ON` before any application code runs.
//! - **Immutable migrations** — SQL files in `src/migrations/` are
//!   `include_str!`'d at compile time. Never edit a shipped migration; always
//!   add a new numbered file. Old databases in `~/.vestige/projects/*/` won't
//!   re-run a mutated migration and will silently diverge.
//! - **Soft-delete only** — no `DELETE FROM memories`. Status flips drive all
//!   lifecycle transitions; FTS sync is handled by triggers in migration 0002.
//! - **Project-scope boundary** — every query that reads memories must filter
//!   by `project_id`. Callers are responsible for passing the correct ID;
//!   nothing in this crate cross-project queries.
//!
//! # Source-of-truth layers
//!
//! Three layers must remain independently serviceable (PRD §9.1):
//!
//! 1. `memory_events` — append-only journal; never updated.
//! 2. `memories` + `memory_representations` — derived interpretation; can be
//!    rebuilt from events.
//! 3. `memory_fts` / `memory_vectors` — disposable acceleration; rebuildable
//!    from layer 2.

mod candidate_ops;
mod embeddings;
mod helpers;
mod memory_ops;
mod project;
mod provenance;
mod trace_ops;

pub use candidate_ops::{CandidateFilter, CandidateHit};
pub use embeddings::{EmbeddingStatus, NewEmbedding, VectorFilter, VectorHit};
pub use provenance::{ProvenanceEvent, SourceReceiptRow};
pub use trace_ops::{NewQueryEvent, QueryEventRow, TraceFilter};
pub use vestige_core::MemoryCounts;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use thiserror::Error;
use tracing::debug;

use vestige_core::{EmbeddingId, ProjectId};

/// Errors produced by the store layer.
///
/// Callers at the CLI boundary should wrap these with `anyhow::Context`.
/// The MCP layer must map them to `{ code, message, retryable }` before
/// returning to agents (PRD §14.3).
#[derive(Debug, Error)]
pub enum StoreError {
    /// A filesystem operation failed (e.g. creating the store directory).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A `rusqlite` call returned an error, including type-conversion failures
    /// when mapping SQLite columns to Rust types.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A migration failed to apply or validate.
    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    /// RFC-3339 timestamp formatting failed (should be unreachable at runtime).
    #[error("time: {0}")]
    Time(#[from] time::error::Format),

    /// Persisted data violated an internal invariant (e.g. wrong-length BLOB,
    /// ID prefix mismatch). Indicates either a writer bug or on-disk damage.
    #[error("data corruption: {0}")]
    Corruption(String),
}

/// Crate-local `Result` alias — wraps [`StoreError`].
pub type Result<T> = std::result::Result<T, StoreError>;

const MIGRATION_INIT: &str = include_str!("migrations/0001_init.sql");
const MIGRATION_FTS: &str = include_str!("migrations/0002_fts.sql");
const MIGRATION_EMBEDDINGS: &str = include_str!("migrations/0003_embeddings.sql");
const MIGRATION_CANDIDATES: &str = include_str!("migrations/0004_candidates.sql");
const MIGRATION_PROVENANCE: &str = include_str!("migrations/0005_provenance.sql");

/// Build the ordered migration set from the embedded SQL files.
///
/// Returns a new [`Migrations`] instance each call; cheap — no I/O, no SQLite
/// work. Call `.validate()` in tests to confirm the SQL parses correctly.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(MIGRATION_INIT),
        M::up(MIGRATION_FTS),
        M::up(MIGRATION_EMBEDDINGS),
        M::up(MIGRATION_CANDIDATES),
        M::up(MIGRATION_PROVENANCE),
    ])
}

/// Handle to the project's SQLite database.
///
/// Each `Store` owns a single `rusqlite::Connection` opened in WAL mode. The
/// connection is not `Send`; callers must open a new `Store` per thread (or
/// per CLI invocation — Vestige has no daemon, PRD §2.3).
///
/// Methods are split across module files by concern:
/// - `project.rs` — upsert and fetch project rows
/// - `memory_ops.rs` — memory CRUD, FTS search, soft-delete, event logging
/// - `embeddings.rs` — vector insert/stale/delete, nearest-neighbour scan
/// - `candidate_ops/` — candidate inbox CRUD, FTS dedup search, lifecycle
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// Open (or create) the SQLite database at `path`.
    ///
    /// - Creates `path`'s parent directory tree if it does not yet exist.
    /// - Sets `journal_mode = WAL` and `foreign_keys = ON`.
    /// - Applies any pending migrations via `rusqlite_migration` (idempotent).
    ///
    /// Fails fast if the file is not a valid SQLite database or any migration
    /// cannot be applied. This is the only constructor; there is no `new`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let m = migrations();
        m.to_latest(&mut conn)?;
        debug!(?path, "store opened");

        Ok(Self { conn, path })
    }

    /// Open the store and set a `PRAGMA busy_timeout` so writers wait politely
    /// under contention instead of returning `SQLITE_BUSY` immediately.
    ///
    /// Intended for the V0.5 daemon, which coexists with CLI/MCP processes via
    /// WAL. Use [`Store::open`] for one-shot CLI invocations where failing fast
    /// is preferable.
    ///
    /// `busy_timeout_ms` is passed to `rusqlite`'s `Connection::busy_timeout`;
    /// SQLite will poll-wait for up to that many milliseconds before surfacing
    /// a busy error to the caller.
    pub fn open_with_busy_timeout(path: impl AsRef<Path>, busy_timeout_ms: u32) -> Result<Self> {
        let store = Self::open(path)?;
        store
            .conn
            .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms.into()))?;
        Ok(store)
    }

    /// Filesystem path of the database file this `Store` was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Shared reference to the underlying `rusqlite::Connection`.
    ///
    /// Internal: schema-inspection escape hatch for `vestige-store`'s own
    /// integration tests. Application crates (`vestige-cli`, `vestige-mcp`,
    /// `vestige-engine`) must use the typed `Store` API and must not depend
    /// on `rusqlite` directly.
    #[doc(hidden)]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Exclusive reference to the underlying `rusqlite::Connection`.
    ///
    /// See [`connection`][Store::connection] for visibility constraints.
    #[doc(hidden)]
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    // === MAINTENANCE API ===

    /// Rebuild the FTS5 shadow tables from the current `memory_representations`
    /// rows. Used by `vestige reindex --fts`.
    ///
    /// Wraps `INSERT INTO memory_fts(memory_fts) VALUES('rebuild')` — the
    /// SQLite-documented FTS5 maintenance command that reconstructs the
    /// inverted index from the source rows. Idempotent.
    pub fn rebuild_fts(&mut self) -> Result<()> {
        self.conn
            .execute("INSERT INTO memory_fts(memory_fts) VALUES('rebuild')", [])?;
        Ok(())
    }

    /// Run `VACUUM` on the database to reclaim free pages and defragment.
    ///
    /// SQLite WAL mode accumulates free pages over time as rows are deleted or
    /// updated. `VACUUM` rebuilds the database file from scratch — it reclaims
    /// space and rewrites the file into a compact form.
    ///
    /// This is a DDL operation; it briefly holds an exclusive lock on the file.
    /// The daemon's `busy_timeout` ensures polite waiting under contention.
    /// Safe to call at any time — it is a no-op if the database is already compact.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// List IDs of pending candidates whose `created_at` predates `cutoff_rfc3339`.
    ///
    /// Returns only `status = 'pending'` rows — approved, rejected, and
    /// superseded candidates are excluded. Used by the daemon's candidate-TTL job
    /// to find stale inbox entries for expiry.
    ///
    /// `cutoff_rfc3339` must be a valid RFC-3339 timestamp string. SQLite compares
    /// it lexicographically against the stored `created_at` values (also RFC-3339),
    /// which is correct because RFC-3339 timestamps sort lexicographically when
    /// normalised to UTC (as stored by `rfc3339()`).
    pub fn list_pending_candidates_older_than(
        &self,
        project_id: &vestige_core::ProjectId,
        cutoff_rfc3339: &str,
    ) -> Result<Vec<vestige_core::CandidateId>> {
        use std::str::FromStr;
        let mut stmt = self.conn.prepare(
            "SELECT id FROM candidate_memories
             WHERE project_id = ?1
               AND status = 'pending'
               AND created_at < ?2
             ORDER BY created_at ASC",
        )?;
        let ids: Vec<vestige_core::CandidateId> = stmt
            .query_map(
                rusqlite::params![project_id.as_str(), cutoff_rfc3339],
                |row| row.get::<_, String>(0),
            )?
            .filter_map(|r| {
                r.ok()
                    .and_then(|s| vestige_core::CandidateId::from_str(&s).ok())
            })
            .collect();
        Ok(ids)
    }

    /// Hard-delete every embedding row (and cascading vector blob) for `project_id`.
    ///
    /// Embeddings are a disposable acceleration layer (PRD §5.3) — clearing them
    /// is safe; they can be regenerated by re-running `vestige embed --all`.
    /// Used by `vestige reindex --embeddings` before re-embedding from scratch.
    /// Returns the number of `memory_embeddings` rows deleted.
    pub fn clear_project_embeddings(&mut self, project_id: &ProjectId) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM memory_embeddings
             WHERE memory_id IN (
                 SELECT id FROM memories WHERE project_id = ?1
             )",
            rusqlite::params![project_id.as_str()],
        )?;
        Ok(deleted)
    }

    // === EMBEDDING API ===

    /// Insert or replace an embedding + vector blob in a single transaction.
    ///
    /// Uses `INSERT OR REPLACE` on the unique index `(representation_id, provider, model)`.
    /// Idempotent: re-embedding the same representation replaces the old row.
    pub fn record_embedding(&mut self, new: &NewEmbedding<'_>) -> Result<EmbeddingId> {
        embeddings::record_embedding(&self.conn, new)
    }

    /// Mark a single embedding stale by its ID.
    pub fn mark_embedding_stale(&mut self, embedding_id: &EmbeddingId) -> Result<()> {
        embeddings::mark_embedding_stale(&self.conn, embedding_id)
    }

    /// Mark all active embeddings for a given representation stale.
    ///
    /// Returns the number of rows affected.
    pub fn mark_representation_embeddings_stale(
        &mut self,
        representation_id: &str,
    ) -> Result<usize> {
        embeddings::mark_representation_embeddings_stale(&self.conn, representation_id)
    }

    /// Hard-delete an embedding row and its vector (FK cascade).
    ///
    /// Embeddings are disposable acceleration — hard delete is acceptable here.
    /// Returns `true` if a row was deleted.
    pub fn delete_embedding(&mut self, embedding_id: &EmbeddingId) -> Result<bool> {
        embeddings::delete_embedding(&self.conn, embedding_id)
    }

    /// Brute-force cosine nearest-neighbour search within the project scope.
    ///
    /// Enforces `project_id` via JOIN — callers cannot bypass this guard.
    /// Only active memories with active embeddings matching `filter` are included.
    pub fn nearest_neighbours(
        &self,
        project_id: &ProjectId,
        query_vec: &[f32],
        k: u32,
        filter: &VectorFilter,
    ) -> Result<Vec<VectorHit>> {
        embeddings::nearest_neighbours(&self.conn, project_id, query_vec, k, filter)
    }

    /// Snapshot of embedding coverage for a project.
    pub fn embedding_status(&self, project_id: &ProjectId) -> Result<EmbeddingStatus> {
        embeddings::embedding_status(&self.conn, project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::ProjectId;

    #[test]
    fn open_runs_migrations_and_creates_db() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("memory.sqlite");
        let store = Store::open(&db).unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn open_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("memory.sqlite");
        Store::open(&db).unwrap();
        Store::open(&db).unwrap();
    }

    #[test]
    fn ensure_project_idempotent() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let id = ProjectId::from_slug("vestige");
        let p1 = store
            .ensure_project(&id, "Vestige", Some("/repo"), None)
            .unwrap();
        let p2 = store
            .ensure_project(&id, "Vestige", Some("/repo"), None)
            .unwrap();
        assert_eq!(p1.id, p2.id);
        assert_eq!(p1.created_at, p2.created_at);
    }

    #[test]
    fn migrations_check_valid() {
        // rusqlite_migration ships a self-check ensuring SQL parses cleanly.
        migrations().validate().unwrap();
    }

    #[test]
    fn open_with_busy_timeout_sets_pragma() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("memory.sqlite");
        let store = Store::open_with_busy_timeout(&path, 5000).unwrap();
        // Verify SQLite honoured the request via the read-back pragma.
        let timeout_ms: i64 = store
            .connection()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert!(
            timeout_ms >= 5000,
            "expected busy_timeout ≥5000 ms, got {timeout_ms}"
        );
    }

    #[test]
    fn vacuum_on_empty_store_succeeds() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        store
            .vacuum()
            .expect("vacuum must succeed on an empty store");
    }

    #[test]
    fn list_pending_candidates_older_than_returns_stale_ids() {
        use vestige_core::{build_candidate_bundle, CandidateId, MemoryType, NewCandidate};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let project_id = ProjectId::from_slug("ttl-test");
        store
            .ensure_project(&project_id, "TTL Test", Some("/tmp"), None)
            .unwrap();

        // Insert two candidates normally.
        let make = |body: &str| {
            build_candidate_bundle(NewCandidate {
                project_id: project_id.clone(),
                proposed_type: MemoryType::Observation,
                body: body.to_string(),
                rationale: None,
                title_override: None,
                importance: 0.5,
                confidence: 0.9,
                source: None,
                duplicate_of_memory_id: None,
                duplicate_of_candidate_id: None,
            })
            .unwrap()
        };

        let bundle_old = make("Old candidate body that should be expired.");
        let bundle_new = make("New candidate body that should survive.");
        let old_id: CandidateId = bundle_old.id.clone();
        let new_id: CandidateId = bundle_new.id.clone();
        store.record_candidate(&bundle_old).unwrap();
        store.record_candidate(&bundle_new).unwrap();

        // Backdate one candidate to 2 days ago using the test helper.
        seed_backdate_candidate(&store, &old_id, "2024-01-01T00:00:00Z");

        // A cutoff of "2024-06-01" should find the backdated candidate.
        let stale = store
            .list_pending_candidates_older_than(&project_id, "2024-06-01T00:00:00Z")
            .unwrap();
        assert_eq!(stale.len(), 1, "only the backdated candidate should appear");
        assert_eq!(stale[0], old_id);

        // The new candidate should not be in the result.
        assert!(!stale.contains(&new_id), "new candidate must not be stale");
    }

    /// Test helper: backdate a candidate's `created_at` column directly.
    ///
    /// This bypasses the normal write path to simulate an old candidate for TTL tests.
    /// Only available in tests; there is no public API for timestamp overrides.
    #[cfg(test)]
    fn seed_backdate_candidate(store: &Store, id: &vestige_core::CandidateId, rfc3339: &str) {
        store
            .connection()
            .execute(
                "UPDATE candidate_memories SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![rfc3339, id.as_str()],
            )
            .expect("backdate candidate created_at");
    }
}
