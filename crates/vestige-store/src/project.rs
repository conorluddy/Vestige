//! `Store` methods for project-level persistence: upsert and fetch.

use std::str::FromStr;

use time::OffsetDateTime;

use vestige_core::{ProjectId, ProjectRecord};

use crate::helpers::{invalid_id_to_sqlite, parse_rfc3339, rfc3339};
use crate::{Result, Store, StoreError};

impl Store {
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
