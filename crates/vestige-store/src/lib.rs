//! SQLite-backed store for Vestige memories.
//!
//! Owns connection management and migrations. Higher-level memory operations
//! live alongside in `vestige-core`'s engine and call into here through the
//! `Store` API.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::debug;
use ulid::Ulid;

use vestige_core::{ProjectId, ProjectRecord};

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
}

pub type Result<T> = std::result::Result<T, StoreError>;

const MIGRATION_INIT: &str = include_str!("migrations/0001_init.sql");
const MIGRATION_FTS: &str = include_str!("migrations/0002_fts.sql");

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(MIGRATION_INIT), M::up(MIGRATION_FTS)])
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
        let now = OffsetDateTime::now_utc();
        let now_str = now
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(StoreError::Time)?;

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

        self.get_project(id).map(|opt| opt.expect("just inserted"))
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
        let now_str = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(StoreError::Time)?;
        self.conn.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, project_id.as_str(), event_type, payload_json, now_str],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryCounts {
    pub active: i64,
    pub deleted: i64,
}

fn row_to_project(row: &rusqlite::Row<'_>) -> Result<ProjectRecord> {
    use std::str::FromStr;

    let id_str: String = row.get(0)?;
    let id = ProjectId::from_str(&id_str).map_err(|e| StoreError::Sqlite(
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)),
    ))?;
    let name: String = row.get(1)?;
    let root_path: Option<String> = row.get(2)?;
    let git_remote: Option<String> = row.get(3)?;
    let created_str: String = row.get(4)?;
    let updated_str: String = row.get(5)?;
    let created_at = OffsetDateTime::parse(&created_str, &time::format_description::well_known::Rfc3339)
        .map_err(|e| StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            4, rusqlite::types::Type::Text, Box::new(e),
        )))?;
    let updated_at = OffsetDateTime::parse(&updated_str, &time::format_description::well_known::Rfc3339)
        .map_err(|e| StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            5, rusqlite::types::Type::Text, Box::new(e),
        )))?;

    Ok(ProjectRecord {
        id,
        name,
        root_path,
        git_remote,
        created_at,
        updated_at,
    })
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
        let p1 = store.ensure_project(&id, "Vestige", Some("/repo"), None).unwrap();
        let p2 = store.ensure_project(&id, "Vestige", Some("/repo"), None).unwrap();
        assert_eq!(p1.id, p2.id);
        assert_eq!(p1.created_at, p2.created_at);
    }

    #[test]
    fn migrations_check_valid() {
        // rusqlite_migration ships a self-check ensuring SQL parses cleanly.
        migrations().validate().unwrap();
    }
}
