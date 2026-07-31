//! `Store` methods for memory CRUD, FTS search, soft-delete, and event logging.
//!
//! # Soft-delete invariant
//!
//! **No `DELETE FROM memories` anywhere in this module tree.** Every lifecycle
//! transition is a status flip (`active` → `deleted` or back). The FTS index
//! stays consistent through SQLite triggers defined in migration 0002:
//! soft-delete fires `memory_after_soft_delete` (drops FTS rows) and restore
//! fires `memory_after_restore` (re-inserts them).
//!
//! # Event journal
//!
//! Every mutating operation appends a row to `memory_events`. The journal is
//! append-only — no event row is ever updated or deleted. It is the canonical
//! audit trail and can reconstruct `memories` if that table were wiped.
//!
//! # FTS search strategy
//!
//! `search_memories` runs the FTS5 `MATCH` query in isolation (no JOIN) to
//! avoid bm25 aggregation limitations in some SQLite builds. Project-scope and
//! status filtering are applied client-side in Rust after the FTS pass, which
//! is acceptable because project DBs are already per-project (PRD §9).
//!
//! # File layout
//!
//! - `mod.rs` — shared `row_to_memory` private helper used by every read path.
//! - `record.rs` — `record_memory`, `record_event`, `append_status_event`.
//! - `lifecycle.rs` — `forget_memory`, `restore_memory` (status flips only).
//! - `fetch.rs` — single-memory reads + representation/source helpers + counts.
//! - `list_search.rs` — bulk list and FTS5 search.

mod fetch;
mod lifecycle;
mod list_search;
mod record;

use std::str::FromStr;

use vestige_core::{Memory, MemoryId, MemoryStatus, MemoryType, ProjectId};

use crate::helpers::parse_rfc3339;
use crate::StoreError;

/// Map a `memories` SELECT row (columns 0–12) into a [`Memory`].
///
/// Column order must match the SELECT list in every caller:
/// `id, project_id, type, status, confidence, importance, created_at, updated_at, deleted_at,
/// recall_count, expand_count, last_recalled_at, superseded_by`.
pub(super) fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let id_str: String = row.get(0)?;
    let project_str: String = row.get(1)?;
    let type_str: String = row.get(2)?;
    let status_str: String = row.get(3)?;
    let confidence: f64 = row.get(4)?;
    let importance: f64 = row.get(5)?;
    let created_str: String = row.get(6)?;
    let updated_str: String = row.get(7)?;
    let deleted_str: Option<String> = row.get(8)?;
    let recall_count: i64 = row.get(9)?;
    let expand_count: i64 = row.get(10)?;
    let last_recalled_str: Option<String> = row.get(11)?;
    let superseded_by_str: Option<String> = row.get(12)?;

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
    let last_recalled_at = match last_recalled_str {
        Some(s) => Some(parse_rfc3339(&s, 11).map_err(|e| match e {
            StoreError::Sqlite(err) => err,
            other => rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(other),
            ),
        })?),
        None => None,
    };
    let superseded_by = match superseded_by_str {
        Some(s) => Some(MemoryId::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
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
        recall_count,
        expand_count,
        last_recalled_at,
        superseded_by,
    })
}
