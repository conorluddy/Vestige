//! Provenance read paths — memory events journal walk and typed source receipt queries.
//!
//! All reads are project-scoped and work for any memory status (active or deleted),
//! which is required by PRD §11.3: `vestige why` must surface the `memory.forgotten`
//! event even for soft-deleted memories.
//!
//! # Two query surfaces
//!
//! - [`ProvenanceEvent`] — one row from `memory_provenance` view (or `memory_events`
//!   for candidates). Used by the engine's `walk_provenance` to build the timeline.
//! - [`SourceReceiptRow`] — one row from `memory_sources` or `candidate_sources`,
//!   with the source row `id` included. Used by `list_sources` in the engine.

use vestige_core::{CandidateId, MemoryId};

use crate::{Result, Store};

// === PUBLIC TYPES ===

/// One event in the journal for a memory or candidate.
///
/// Projected from `memory_provenance` (for memories) or from `memory_events` via
/// the `memory_id` index (for candidates, which store `candidate_id` in payload).
#[derive(Debug, Clone)]
pub struct ProvenanceEvent {
    /// `evt_<ULID>` — the event row's primary key.
    pub event_id: String,
    /// Dot-namespaced event type (e.g. `"memory.recorded"`, `"memory.forgotten"`).
    pub event_type: String,
    /// Raw JSON payload — the full capture payload or status-transition metadata.
    pub payload_json: Option<String>,
    /// RFC-3339 timestamp when the event was written.
    pub event_at: String,
}

/// One source row with its database ID exposed.
///
/// `memory_sources` and `candidate_sources` both carry an `id` column (`src_<ULID>`)
/// that is not surfaced by the standard `SourceRow` type. Provenance queries expose
/// it so `vestige why` and `vestige sources` can reference sources by ID in output.
#[derive(Debug, Clone)]
pub struct SourceReceiptRow {
    /// `src_<ULID>` — the source row's primary key.
    pub source_id: String,
    /// Typed evidence category (`"file"`, `"agent_session"`, `"candidate"`, …).
    pub source_type: String,
    /// Stable locator (file path, URL, session ref, candidate id, …).
    pub source_ref: Option<String>,
    /// Stored content snippet (may be shorter than original if truncated).
    pub source_content: Option<String>,
}

// === STORE METHODS ===

impl Store {
    /// Fetch all journal events for a memory, ordered by `created_at`.
    ///
    /// Reads from the `memory_provenance` view (migration 0005) using the indexed
    /// `memory_id` column. Works for any memory status including soft-deleted rows.
    /// Returns an empty `Vec` when no events are found (e.g. very fresh memories
    /// before the first status transition).
    pub fn fetch_memory_events(&self, id: &MemoryId) -> Result<Vec<ProvenanceEvent>> {
        let mut stmt = self.connection().prepare(
            "SELECT event_id, event_type, payload_json, event_at
             FROM memory_provenance
             WHERE memory_id = ?1
             ORDER BY event_at ASC",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![id.as_str()], |row| {
                let event_id: String = row.get(0)?;
                let event_type: String = row.get(1)?;
                let payload_json: Option<String> = row.get(2)?;
                let event_at: String = row.get(3)?;
                Ok(ProvenanceEvent {
                    event_id,
                    event_type,
                    payload_json,
                    event_at,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Filter out NULL event rows (LEFT JOIN produces one NULL row when there
        // are no events yet). A NULL event_id indicates the memory exists but has
        // no journal entries.
        Ok(rows
            .into_iter()
            .filter(|e| !e.event_id.is_empty())
            .collect())
    }

    /// Fetch all journal events for a candidate, ordered by `created_at`.
    ///
    /// Reads directly from `memory_events` filtered by `event_type LIKE 'candidate.%'`
    /// and the candidate ID embedded in `payload_json`. Uses `json_extract` — slightly
    /// less efficient than the indexed `memory_id` column, but candidates are low-volume
    /// and the view is memory-scoped. The `memory_id` index on `memory_events` does not
    /// cover candidates (their events carry `candidate_id` in the payload, not `memory_id`).
    pub fn fetch_candidate_events(&self, id: &CandidateId) -> Result<Vec<ProvenanceEvent>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, event_type, payload_json, created_at
             FROM memory_events
             WHERE json_extract(payload_json, '$.candidate_id') = ?1
             ORDER BY created_at ASC",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![id.as_str()], |row| {
                let event_id: String = row.get(0)?;
                let event_type: String = row.get(1)?;
                let payload_json: Option<String> = row.get(2)?;
                let event_at: String = row.get(3)?;
                Ok(ProvenanceEvent {
                    event_id,
                    event_type,
                    payload_json,
                    event_at,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Fetch all `memory_sources` rows for a memory, with source IDs exposed.
    ///
    /// Returns rows in insertion order (`created_at ASC`). Works for any memory
    /// status. Optionally filtered by `source_type` — pass `None` for all sources.
    pub fn fetch_memory_sources(
        &self,
        id: &MemoryId,
        kind_filter: Option<&str>,
    ) -> Result<Vec<SourceReceiptRow>> {
        if let Some(kind) = kind_filter {
            let mut stmt = self.connection().prepare(
                "SELECT id, source_type, source_ref, source_content
                 FROM memory_sources
                 WHERE memory_id = ?1 AND source_type = ?2
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id.as_str(), kind], row_to_source_receipt)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        } else {
            let mut stmt = self.connection().prepare(
                "SELECT id, source_type, source_ref, source_content
                 FROM memory_sources
                 WHERE memory_id = ?1
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id.as_str()], row_to_source_receipt)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }

    /// Fetch all `candidate_sources` rows for a candidate, with source IDs exposed.
    ///
    /// Returns rows in insertion order (`created_at ASC`). Optionally filtered by
    /// `source_type` — pass `None` for all sources.
    pub fn fetch_candidate_sources_with_ids(
        &self,
        id: &CandidateId,
        kind_filter: Option<&str>,
    ) -> Result<Vec<SourceReceiptRow>> {
        if let Some(kind) = kind_filter {
            let mut stmt = self.connection().prepare(
                "SELECT id, source_type, source_ref, source_content
                 FROM candidate_sources
                 WHERE candidate_id = ?1 AND source_type = ?2
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id.as_str(), kind], row_to_source_receipt)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        } else {
            let mut stmt = self.connection().prepare(
                "SELECT id, source_type, source_ref, source_content
                 FROM candidate_sources
                 WHERE candidate_id = ?1
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id.as_str()], row_to_source_receipt)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }
}

// === PRIVATE HELPERS ===

fn row_to_source_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceReceiptRow> {
    Ok(SourceReceiptRow {
        source_id: row.get(0)?,
        source_type: row.get(1)?,
        source_ref: row.get(2)?,
        source_content: row.get(3)?,
    })
}
