//! SQLite-backed store for Vestige memories.
//!
//! Owns connection management and migrations. Higher-level memory operations
//! live alongside in `vestige-core`'s engine and call into here through the
//! `Store` API.

mod embeddings;
mod helpers;
mod memory_ops;
mod project;

pub use embeddings::{EmbeddingStatus, NewEmbedding, VectorFilter, VectorHit};
pub use memory_ops::MemoryCounts;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use thiserror::Error;
use tracing::debug;

use vestige_core::{EmbeddingId, ProjectId};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("time: {0}")]
    Time(#[from] time::error::Format),

    #[error("data corruption: {0}")]
    Corruption(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

const MIGRATION_INIT: &str = include_str!("migrations/0001_init.sql");
const MIGRATION_FTS: &str = include_str!("migrations/0002_fts.sql");
const MIGRATION_EMBEDDINGS: &str = include_str!("migrations/0003_embeddings.sql");

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(MIGRATION_INIT),
        M::up(MIGRATION_FTS),
        M::up(MIGRATION_EMBEDDINGS),
    ])
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// Open or create the SQLite store at `path`, ensuring the parent
    /// directory exists and migrations are applied.
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
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
}
