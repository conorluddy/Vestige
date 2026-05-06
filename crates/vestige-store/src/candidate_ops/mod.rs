//! `Store` methods for candidate CRUD, FTS dedup search, and audit events.
//!
//! # Design notes
//!
//! Candidates are the V0.2 assimilation inbox layer. They share the `memory_events`
//! journal (new `candidate.*` event types) but live in their own tables:
//! `candidate_memories`, `candidate_sources`, `candidate_fts`.
//!
//! **Soft-delete only** — no `DELETE FROM candidate_memories`. All lifecycle
//! transitions are status flips. The `candidate_fts_after_update` trigger in
//! migration 0004 removes rows from the FTS index automatically when status
//! leaves `'pending'`.
//!
//! **Project-scope boundary** — every read takes `&ProjectId` and filters in SQL.
//! Callers may not cross project boundaries here.
//!
//! # File layout
//!
//! - `mod.rs` — shared `row_to_candidate` helper used by every read path.
//! - `record.rs` — `record_candidate` (insert + sources + audit event, one tx).
//! - `lifecycle.rs` — `mark_candidate_approved`, `mark_candidate_rejected`.
//! - `fetch.rs` — `get_candidate`, `fetch_candidate_sources`.
//! - `list_search.rs` — `list_candidates`, `search_candidates_lexical`.

mod fetch;
mod lifecycle;
mod list_search;
mod record;

pub use list_search::{CandidateFilter, CandidateHit};

use std::str::FromStr;

use vestige_core::{
    Candidate, CandidateId, CandidateSource, CandidateStatus, MemoryId, MemoryType, ProjectId,
};

use crate::helpers::parse_rfc3339;
use crate::StoreError;

/// Map a `candidate_memories` SELECT row (columns 0–18) into a [`Candidate`].
///
/// Column order must match every SELECT list in this module:
/// `id, project_id, proposed_type, status, title, one_liner, summary, full_body,
///  rationale, confidence, importance, duplicate_of_memory_id, duplicate_of_candidate_id,
///  approved_memory_id, rejection_reason, review_note, created_at, updated_at, reviewed_at`.
pub(super) fn row_to_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<Candidate> {
    let id_str: String = row.get(0)?;
    let project_str: String = row.get(1)?;
    let type_str: String = row.get(2)?;
    let status_str: String = row.get(3)?;
    let title: String = row.get(4)?;
    let one_liner: String = row.get(5)?;
    let summary: Option<String> = row.get(6)?;
    let full_body: String = row.get(7)?;
    let rationale: Option<String> = row.get(8)?;
    let confidence: f64 = row.get(9)?;
    let importance: f64 = row.get(10)?;
    let dup_mem_str: Option<String> = row.get(11)?;
    let dup_cand_str: Option<String> = row.get(12)?;
    let approved_mem_str: Option<String> = row.get(13)?;
    let rejection_reason_str: Option<String> = row.get(14)?;
    let review_note: Option<String> = row.get(15)?;
    let created_str: String = row.get(16)?;
    let updated_str: String = row.get(17)?;
    let reviewed_str: Option<String> = row.get(18)?;

    let id = CandidateId::from_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let project_id = ProjectId::from_str(&project_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let proposed_type = MemoryType::from_str(&type_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status = CandidateStatus::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let duplicate_of_memory_id = dup_mem_str
        .map(|s| MemoryId::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let duplicate_of_candidate_id = dup_cand_str
        .map(|s| CandidateId::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let approved_memory_id = approved_mem_str
        .map(|s| MemoryId::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let rejection_reason = rejection_reason_str
        .map(|s| vestige_core::RejectionReason::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(14, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let created_at = parse_rfc3339(&created_str, 16).map_err(|e| match e {
        StoreError::Sqlite(err) => err,
        other => rusqlite::Error::FromSqlConversionFailure(
            16,
            rusqlite::types::Type::Text,
            Box::new(other),
        ),
    })?;
    let updated_at = parse_rfc3339(&updated_str, 17).map_err(|e| match e {
        StoreError::Sqlite(err) => err,
        other => rusqlite::Error::FromSqlConversionFailure(
            17,
            rusqlite::types::Type::Text,
            Box::new(other),
        ),
    })?;
    let reviewed_at = match reviewed_str {
        Some(s) => Some(parse_rfc3339(&s, 18).map_err(|e| match e {
            StoreError::Sqlite(err) => err,
            other => rusqlite::Error::FromSqlConversionFailure(
                18,
                rusqlite::types::Type::Text,
                Box::new(other),
            ),
        })?),
        None => None,
    };

    Ok(Candidate {
        id,
        project_id,
        proposed_type,
        status,
        title,
        one_liner,
        summary,
        full_body,
        rationale,
        confidence: confidence as f32,
        importance: importance as f32,
        duplicate_of_memory_id,
        duplicate_of_candidate_id,
        approved_memory_id,
        rejection_reason,
        review_note,
        created_at,
        updated_at,
        reviewed_at,
        sources: vec![],
    })
}

/// Map a `candidate_sources` SELECT row into a [`CandidateSource`].
///
/// Column order: `source_type, source_ref, source_content`.
pub(super) fn row_to_candidate_source(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CandidateSource> {
    let source_type: String = row.get(0)?;
    let source_ref: Option<String> = row.get(1)?;
    let source_content: Option<String> = row.get(2)?;
    Ok(CandidateSource {
        source_type,
        source_ref,
        source_content,
        truncated: false,
    })
}
