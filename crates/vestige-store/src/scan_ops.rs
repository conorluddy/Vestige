//! `session_scan_cursors` read and write paths.
//!
//! Each row records the watermark (byte offset) up to which a given source
//! (e.g. `"claude_code"`) has scanned a specific file for a project. On the
//! next scan pass the caller reads the cursor, seeks to `last_offset`, and
//! resumes from there — ensuring no transcript content is processed twice and
//! no content is missed after a restart.
//!
//! `INSERT OR REPLACE` is used for writes so re-scanning a file updates the
//! existing watermark in place rather than inserting a second row.

use std::str::FromStr;

use time::OffsetDateTime;

use vestige_core::ProjectId;

use crate::helpers::{invalid_id_to_sqlite, rfc3339};
use crate::{Result, Store};

// === PUBLIC TYPES ===

/// One row from `session_scan_cursors`.
///
/// Returned by [`Store::get_scan_cursor`]; produced and persisted by
/// [`Store::record_scan_cursor`].
#[derive(Debug, Clone)]
pub struct ScanCursorRow {
    /// Session source identifier: `"claude_code"` or `"codex"`.
    pub source: String,
    /// Absolute or repo-relative path of the scanned file.
    pub file_path: String,
    /// Project this cursor belongs to.
    pub project_id: ProjectId,
    /// Byte offset (or line number, depending on source) scanned through.
    pub last_offset: i64,
    /// RFC-3339 UTC timestamp of the last scan.
    pub last_scanned_at: String,
}

// === STORE IMPL ===

impl Store {
    /// Fetch the scan cursor for a `(source, file_path)` pair.
    ///
    /// Returns `Ok(None)` when no cursor has been recorded yet — callers
    /// should then scan from byte offset 0.
    pub fn get_scan_cursor(&self, source: &str, file_path: &str) -> Result<Option<ScanCursorRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source, file_path, project_id, last_offset, last_scanned_at
             FROM session_scan_cursors
             WHERE source = ?1 AND file_path = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![source, file_path])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_scan_cursor(row)?))
        } else {
            Ok(None)
        }
    }

    /// Upsert a scan cursor, stamping `last_scanned_at` with the current UTC
    /// time.
    ///
    /// Uses `INSERT OR REPLACE` so re-scanning the same file updates the
    /// watermark in place. Callers should pass the highest byte offset
    /// processed in this scan pass; a lower value would regress the watermark.
    pub fn record_scan_cursor(
        &self,
        source: &str,
        file_path: &str,
        project_id: &ProjectId,
        last_offset: i64,
    ) -> Result<()> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        self.conn.execute(
            "INSERT OR REPLACE INTO session_scan_cursors
                 (source, file_path, project_id, last_offset, last_scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![source, file_path, project_id.as_str(), last_offset, now_str],
        )?;
        Ok(())
    }
}

// === PRIVATE HELPERS ===

fn row_to_scan_cursor(row: &rusqlite::Row<'_>) -> Result<ScanCursorRow> {
    let source: String = row.get(0)?;
    let file_path: String = row.get(1)?;
    let project_id_str: String = row.get(2)?;
    let project_id = ProjectId::from_str(&project_id_str).map_err(invalid_id_to_sqlite)?;
    let last_offset: i64 = row.get(3)?;
    let last_scanned_at: String = row.get(4)?;

    Ok(ScanCursorRow {
        source,
        file_path,
        project_id,
        last_offset,
        last_scanned_at,
    })
}
