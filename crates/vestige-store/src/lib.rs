//! SQLite-backed store for Vestige memories.
//!
//! Owns connection management and migrations. Higher-level memory operations
//! live alongside in `vestige-core`'s engine and call into here through the
//! `Store` API.

mod embeddings;

pub use embeddings::{EmbeddingStatus, NewEmbedding, VectorFilter, VectorHit};

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::debug;
use ulid::Ulid;

use std::str::FromStr;

use vestige_core::{
    EmbeddingId, FetchedMemory, ListFilter, Memory, MemoryBundle, MemoryId, MemoryStatus,
    MemoryType, ProjectId, ProjectRecord, RepresentationDepth, RepresentationRow, SearchFilter,
    SearchHit, SourceRow,
};

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

    /// Insert a project row if it doesn't already exist. Idempotent — used by
    /// `vestige init`.
    pub fn ensure_project(
        &mut self,
        id: &ProjectId,
        name: &str,
        root_path: Option<&str>,
        git_remote: Option<&str>,
    ) -> Result<ProjectRecord> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;

        self.conn.execute(
            "INSERT INTO projects (id, name, root_path, git_remote, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                root_path = excluded.root_path,
                git_remote = excluded.git_remote,
                updated_at = excluded.updated_at",
            rusqlite::params![id.as_str(), name, root_path, git_remote, now_str],
        )?;

        self.get_project(id)?.ok_or_else(|| {
            StoreError::Corruption(format!("project {id} missing immediately after upsert"))
        })
    }

    pub fn get_project(&self, id: &ProjectId) -> Result<Option<ProjectRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, root_path, git_remote, created_at, updated_at
             FROM projects WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_project(row)?))
        } else {
            Ok(None)
        }
    }

    /// Counts memories grouped by status. Used by `vestige status`.
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

    /// Append a structured event to `memory_events`.
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

    /// Persist a full memory bundle (memory row + four representation rows +
    /// optional source row) atomically and append a `memory.recorded` event.
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

    /// Fetch a memory with all joined representations and sources.
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

    /// List memories for a project. Excludes deleted by default.
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

    /// Soft-delete (`forget`) a memory: flip status, set `deleted_at`. The
    /// FTS sync trigger drops its rows from the index. Returns whether the
    /// row was found in `active` state.
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

    /// Restore a soft-deleted memory. The FTS restore trigger re-indexes its
    /// representations. Returns whether the row was found in `deleted` state.
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

    fn append_status_event(&self, id: &MemoryId, event_type: &str, when: &str) -> Result<()> {
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

    fn fetch_memory_row(&self, id: &MemoryId) -> Result<Option<Memory>> {
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

    fn fetch_representations(&self, id: &MemoryId) -> Result<Vec<RepresentationRow>> {
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

    fn fetch_sources(&self, id: &MemoryId) -> Result<Vec<SourceRow>> {
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

#[derive(Debug, Clone, Copy)]
pub struct MemoryCounts {
    pub active: i64,
    pub deleted: i64,
}

fn row_to_project(row: &rusqlite::Row<'_>) -> Result<ProjectRecord> {
    let id_str: String = row.get(0)?;
    let id = ProjectId::from_str(&id_str).map_err(invalid_id_to_sqlite)?;
    let name: String = row.get(1)?;
    let root_path: Option<String> = row.get(2)?;
    let git_remote: Option<String> = row.get(3)?;
    let created_str: String = row.get(4)?;
    let updated_str: String = row.get(5)?;

    Ok(ProjectRecord {
        id,
        name,
        root_path,
        git_remote,
        created_at: parse_rfc3339(&created_str, 4)?,
        updated_at: parse_rfc3339(&updated_str, 5)?,
    })
}

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

fn rfc3339(t: OffsetDateTime) -> Result<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .map_err(StoreError::Time)
}

fn parse_rfc3339(s: &str, col: usize) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|e| {
        StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            col,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))
    })
}

fn invalid_id_to_sqlite<E: std::error::Error + Send + Sync + 'static>(e: E) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
