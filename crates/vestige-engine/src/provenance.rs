//! Provenance walks and source receipt listing for `vestige why` and `vestige sources`.
//!
//! These functions sit in `vestige-engine` (not `vestige-core`) because they need
//! direct `Store` access for journal and source queries — core must stay free of
//! `rusqlite`. Both CLI and MCP delegate here; neither duplicates the logic.
//!
//! # Design
//!
//! Two public functions correspond to the two CLI commands:
//!
//! - [`walk_provenance`] — full templated walk for a memory or candidate.
//! - [`list_sources`] — raw source receipts, optionally filtered by kind.
//!
//! Both accept a [`SubjectId`] that encodes whether the caller is asking about a
//! memory or a candidate. The engine dispatches accordingly.

use serde::{Deserialize, Serialize};
use vestige_core::{CandidateId, MemoryId, ProjectId, SourceKind};
use vestige_store::{ProvenanceEvent, SourceReceiptRow, Store};

use crate::error::{EngineError, Result};

// === PUBLIC TYPES ===

/// Parsed prefix of the ID the user supplied — either a memory or a candidate.
#[derive(Debug, Clone)]
pub enum SubjectId {
    Memory(MemoryId),
    Candidate(CandidateId),
}

impl SubjectId {
    /// Parse `s` by prefix: `mem_` → `Memory`, `cand_` → `Candidate`.
    /// Returns an error for any other prefix or malformed string.
    pub fn parse(s: &str) -> Result<Self> {
        if s.starts_with("mem_") {
            let id = s.parse::<MemoryId>().map_err(EngineError::Core)?;
            Ok(SubjectId::Memory(id))
        } else if s.starts_with("cand_") {
            let id = s.parse::<CandidateId>().map_err(EngineError::Core)?;
            Ok(SubjectId::Candidate(id))
        } else {
            Err(EngineError::Validation {
                message: format!("expected a `mem_<ULID>` or `cand_<ULID>` id, got `{s}`"),
            })
        }
    }
}

/// A journal event row, safe for serialisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEntry {
    /// `evt_<ULID>` primary key.
    pub event_id: String,
    /// Dot-namespaced event type, e.g. `"memory.recorded"`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// RFC-3339 timestamp.
    pub at: String,
    /// Full JSON payload from the event row, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// A source receipt row, safe for serialisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceReceipt {
    /// `src_<ULID>` primary key.
    pub id: String,
    /// Typed evidence kind — always a valid [`SourceKind`] string on write;
    /// any stored string on read (forward-compat).
    pub kind: String,
    /// Stable locator (path, URL, cand id, session ref, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Stored content snippet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Whether the content was truncated to the 2 KiB cap.
    pub truncated: bool,
}

/// Provenance walk for a memory — the primary output of `vestige why <mem_id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenance {
    /// The owning memory's events (recorded, forgotten, restored, …).
    pub events: Vec<EventEntry>,
    /// Reverse-provenance link to the originating candidate, if this memory was
    /// promoted from the assimilation inbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<CandidateProvenance>,
    /// Source receipts attached directly to the memory.
    pub sources: Vec<SourceReceipt>,
}

/// Provenance walk for a candidate — used both standalone and nested in [`MemoryProvenance`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateProvenance {
    /// The candidate's own identifier.
    pub candidate_id: String,
    /// The candidate's journal events (proposed, approved, rejected, …).
    pub events: Vec<EventEntry>,
    /// Source receipts attached to the candidate.
    pub sources: Vec<SourceReceipt>,
}

/// Full provenance walk returned by [`walk_provenance`].
///
/// Matches the shape documented in PRD §13.1 / §10.2:
/// ```json
/// {
///   "memory_id":     "mem_...",   // or "candidate_id" for candidate subjects
///   "type":          "decision",
///   "status":        "active",
///   "provenance":    { ... },
///   "status_history": [...]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceWalk {
    /// The subject ID exactly as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_id: Option<String>,
    /// Set when the subject is a candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    /// Semantic type of the memory or candidate.
    #[serde(rename = "type")]
    pub subject_type: String,
    /// Lifecycle status of the subject (`"active"`, `"deleted"`, `"pending"`, …).
    pub status: String,
    /// Full provenance walk (mirrors `vestige_expand depth=provenance`).
    pub provenance: serde_json::Value,
    /// Status-transition events in chronological order (duplicates `provenance.events`
    /// for the PRD §13.1 top-level field).
    pub status_history: Vec<EventEntry>,
}

/// Typed source-listing result returned by [`list_sources`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceListing {
    /// The owner — `mem_<ULID>` or `cand_<ULID>`.
    pub owner_id: String,
    /// Whether the owner is a `"memory"` or `"candidate"`.
    pub owner_kind: String,
    /// All matching source receipts.
    pub sources: Vec<SourceReceipt>,
}

// === PUBLIC API ===

/// Walk the provenance of a memory or candidate.
///
/// - For memories: reads `memory_provenance` view + `memory_sources` + optional
///   candidate back-reference (discovered via `SourceKind::Candidate` source rows).
/// - For candidates: reads `memory_events` for candidate-scoped events + `candidate_sources`.
///
/// Works for soft-deleted memories — the `memory.forgotten` event appears in the timeline.
pub fn walk_provenance(
    store: &Store,
    project_id: &ProjectId,
    subject: &SubjectId,
) -> Result<ProvenanceWalk> {
    match subject {
        SubjectId::Memory(mem_id) => walk_memory_provenance(store, project_id, mem_id),
        SubjectId::Candidate(cand_id) => walk_candidate_provenance(store, project_id, cand_id),
    }
}

/// List source receipts for a memory or candidate, optionally filtered by kind.
///
/// `kind_filter` is validated via [`SourceKind::parse`] on write paths but here
/// it is passed through as a raw string for read compatibility. Unknown kind values
/// are rejected before reaching the store to surface a user-friendly error.
pub fn list_sources(
    store: &Store,
    project_id: &ProjectId,
    subject: &SubjectId,
    kind_filter: Option<&str>,
) -> Result<SourceListing> {
    // Validate the filter kind so unrecognised values produce a clear error before
    // touching the DB. We use SourceKind::parse (write-strict) so typos are caught.
    if let Some(kind) = kind_filter {
        SourceKind::parse(kind)?;
    }

    match subject {
        SubjectId::Memory(mem_id) => {
            // Verify the memory is in scope (any status).
            let memory = store
                .get_memory(mem_id)?
                .ok_or_else(|| EngineError::Validation {
                    message: format!("memory not found: `{mem_id}`"),
                })?;
            if &memory.memory.project_id != project_id {
                return Err(EngineError::OutOfScope);
            }

            let rows = store.fetch_memory_sources(mem_id, kind_filter)?;
            let sources = rows.into_iter().map(source_receipt_from_row).collect();
            Ok(SourceListing {
                owner_id: mem_id.to_string(),
                owner_kind: "memory".to_string(),
                sources,
            })
        }
        SubjectId::Candidate(cand_id) => {
            let candidate =
                store
                    .get_candidate(cand_id)?
                    .ok_or_else(|| EngineError::Validation {
                        message: format!("candidate not found: `{cand_id}`"),
                    })?;
            if &candidate.project_id != project_id {
                return Err(EngineError::OutOfScope);
            }

            let rows = store.fetch_candidate_sources_with_ids(cand_id, kind_filter)?;
            let sources = rows.into_iter().map(source_receipt_from_row).collect();
            Ok(SourceListing {
                owner_id: cand_id.to_string(),
                owner_kind: "candidate".to_string(),
                sources,
            })
        }
    }
}

// === PRIVATE HELPERS ===

fn walk_memory_provenance(
    store: &Store,
    project_id: &ProjectId,
    mem_id: &MemoryId,
) -> Result<ProvenanceWalk> {
    // Fetch the memory row (any status including deleted).
    let fetched = store
        .get_memory(mem_id)?
        .ok_or_else(|| EngineError::Validation {
            message: format!("memory not found: `{mem_id}`"),
        })?;
    if &fetched.memory.project_id != project_id {
        return Err(EngineError::OutOfScope);
    }

    // Fetch journal events via memory_provenance view.
    let raw_events = store.fetch_memory_events(mem_id)?;
    let events: Vec<EventEntry> = raw_events.iter().map(event_entry_from_row).collect();

    // Fetch sources for this memory.
    let source_rows = store.fetch_memory_sources(mem_id, None)?;
    let sources: Vec<SourceReceipt> = source_rows
        .into_iter()
        .map(source_receipt_from_row)
        .collect();

    // Discover candidate back-reference: look for a SourceKind::Candidate row.
    // The candidate ID is stored as the `source_ref`.
    let candidate_provenance = sources
        .iter()
        .find(|s| s.kind == SourceKind::Candidate.as_str())
        .and_then(|s| s.source_ref.as_deref())
        .and_then(|cand_str| cand_str.parse::<CandidateId>().ok())
        .and_then(|cand_id| {
            // Best-effort: if the candidate is not found or is out of scope, skip it.
            fetch_candidate_provenance_inner(store, &cand_id).ok()
        });

    let provenance = MemoryProvenance {
        events: events.clone(),
        candidate: candidate_provenance,
        sources,
    };

    let status_str = fetched.memory.status.as_str().to_string();
    let type_str = fetched.memory.r#type.as_str().to_string();

    Ok(ProvenanceWalk {
        memory_id: Some(mem_id.to_string()),
        candidate_id: None,
        subject_type: type_str,
        status: status_str,
        provenance: serde_json::to_value(&provenance).unwrap_or(serde_json::Value::Null),
        status_history: events,
    })
}

fn walk_candidate_provenance(
    store: &Store,
    project_id: &ProjectId,
    cand_id: &CandidateId,
) -> Result<ProvenanceWalk> {
    let candidate = store
        .get_candidate(cand_id)?
        .ok_or_else(|| EngineError::Validation {
            message: format!("candidate not found: `{cand_id}`"),
        })?;
    if &candidate.project_id != project_id {
        return Err(EngineError::OutOfScope);
    }

    let cand_prov = fetch_candidate_provenance_inner(store, cand_id)?;
    let events_clone = cand_prov.events.clone();

    let status_str = candidate.status.as_str().to_string();
    let type_str = candidate.proposed_type.as_str().to_string();

    Ok(ProvenanceWalk {
        memory_id: None,
        candidate_id: Some(cand_id.to_string()),
        subject_type: type_str,
        status: status_str,
        provenance: serde_json::to_value(&cand_prov).unwrap_or(serde_json::Value::Null),
        status_history: events_clone,
    })
}

/// Fetch the candidate provenance inner struct (reused when embedding inside a memory walk).
fn fetch_candidate_provenance_inner(
    store: &Store,
    cand_id: &CandidateId,
) -> Result<CandidateProvenance> {
    let raw_events = store.fetch_candidate_events(cand_id)?;
    let events: Vec<EventEntry> = raw_events.iter().map(event_entry_from_row).collect();

    let source_rows = store.fetch_candidate_sources_with_ids(cand_id, None)?;
    let sources: Vec<SourceReceipt> = source_rows
        .into_iter()
        .map(source_receipt_from_row)
        .collect();

    Ok(CandidateProvenance {
        candidate_id: cand_id.to_string(),
        events,
        sources,
    })
}

fn event_entry_from_row(e: &ProvenanceEvent) -> EventEntry {
    let payload = e
        .payload_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    EventEntry {
        event_id: e.event_id.clone(),
        event_type: e.event_type.clone(),
        at: e.event_at.clone(),
        payload,
    }
}

fn source_receipt_from_row(r: SourceReceiptRow) -> SourceReceipt {
    // Truncated is not persisted (it is a build-time annotation); we approximate it
    // by checking whether the content length approaches the 2 KiB cap.
    let truncated = r
        .source_content
        .as_deref()
        .map(|c| c.len() >= vestige_core::SOURCE_SNIPPET_MAX_BYTES.saturating_sub(4))
        .unwrap_or(false);

    SourceReceipt {
        id: r.source_id,
        kind: r.source_type,
        source_ref: r.source_ref,
        content: r.source_content,
        truncated,
    }
}
