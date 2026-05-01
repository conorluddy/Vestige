//! Canonical wire-level models shared across all Vestige crates.
//!
//! These are the structs and enums that `vestige-store` persists, `vestige-cli`
//! formats, and `vestige-mcp` serialises over JSON-RPC. Keep them free of any
//! persistence or transport detail — no `rusqlite`, no `clap`, no `rmcp`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;

use crate::error::CoreError;
use crate::ids::{MemoryId, ProjectId};

// === ENUMERATIONS ===

/// Semantic classification of a memory, used to bias ranking and context
/// assembly. Serialises as snake_case (e.g. `"project_summary"`).
///
/// Type boosts in [`composite_score`](crate::composite_score) and
/// [`merge_hits`](crate::merge_hits) give `ProjectSummary` and `Decision`
/// higher priority than `Note` or `Observation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// A factual observation about code, behaviour, or the environment.
    Observation,
    /// A free-form note without a stronger semantic label.
    Note,
    /// A deliberate architectural or design decision. Gets a ranking boost.
    Decision,
    /// A stated preference (coding style, tooling, UX). Influences future work.
    Preference,
    /// High-level project state — goal, status, key choices. One per project,
    /// surfaced first in context packs. Gets the highest ranking boost.
    ProjectSummary,
    /// An unresolved question to revisit. Surfaced in the "Open questions"
    /// context section.
    OpenQuestion,
}

impl MemoryType {
    /// Return the canonical lowercase string for SQL storage and JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Note => "note",
            Self::Decision => "decision",
            Self::Preference => "preference",
            Self::ProjectSummary => "project_summary",
            Self::OpenQuestion => "open_question",
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryType {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "observation" => Ok(Self::Observation),
            "note" => Ok(Self::Note),
            "decision" => Ok(Self::Decision),
            "preference" => Ok(Self::Preference),
            "project_summary" => Ok(Self::ProjectSummary),
            "open_question" => Ok(Self::OpenQuestion),
            other => Err(CoreError::InvalidMemoryType(other.to_string())),
        }
    }
}

/// Soft-delete status for a memory row. `vestige forget` sets `Deleted`;
/// `vestige restore` sets it back to `Active`. No row is ever physically
/// removed — see CLAUDE.md "Soft-delete only" rule.
///
/// Deleted memories are excluded from search and context packs by default
/// but are preserved in the `memory_events` journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Normal, searchable state.
    Active,
    /// Soft-deleted — excluded from search. Restorable via `vestige restore`.
    Deleted,
}

impl MemoryStatus {
    /// Return the canonical lowercase string for SQL storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

impl FromStr for MemoryStatus {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "deleted" => Ok(Self::Deleted),
            other => Err(CoreError::InvalidMemoryStatus(other.to_string())),
        }
    }
}

/// Progressive disclosure depth (PRD §5.2). `L0` (handle) is just the id and
/// is implicit — these four depths are the stored representations.
///
/// Ordered from shortest to longest:
/// `OneLiner` < `Summary` < `Compressed` < `Full`.
///
/// Use [`pick_representation`](crate::pick_representation) to select the
/// appropriate [`RepresentationRow`](crate::RepresentationRow) from a
/// [`FetchedMemory`](crate::FetchedMemory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationDepth {
    /// Single sentence — fits in a list or search result card.
    OneLiner,
    /// Full body as submitted; human-readable paragraph or two.
    Summary,
    /// Body compressed for token budget (V0: same as `Summary`; LLM pass deferred).
    Compressed,
    /// Verbatim full body without compression.
    Full,
}

impl RepresentationDepth {
    /// Return the canonical snake_case string for SQL storage and JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OneLiner => "one_liner",
            Self::Summary => "summary",
            Self::Compressed => "compressed",
            Self::Full => "full",
        }
    }
}

impl FromStr for RepresentationDepth {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "one_liner" | "oneliner" => Ok(Self::OneLiner),
            "summary" => Ok(Self::Summary),
            "compressed" | "compressed_body" => Ok(Self::Compressed),
            "full" | "full_body" => Ok(Self::Full),
            other => Err(CoreError::InvalidDepth(other.to_string())),
        }
    }
}

// === RECORD TYPES ===

/// A project row from `~/.vestige/projects/<id>/memory.sqlite`.
///
/// Projects are scoped per repo via `.vestige/config.toml`. The `id` is
/// derived from the git remote hash or repo-path hash (PRD §9.3) so it is
/// stable across machines given the same remote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    /// Stable project identifier — `proj_<slug-or-ULID>`.
    pub id: ProjectId,
    /// Human-readable name, set via `vestige init --name`.
    pub name: String,
    /// Absolute path to the repo root when the project was initialised.
    pub root_path: Option<String>,
    /// Git remote URL used to derive the project ID.
    pub git_remote: Option<String>,
    /// When the project row was first created (RFC3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When the project row was last mutated (RFC3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Core memory row — the persistent unit of recall.
///
/// A `Memory` is always accompanied by one or more [`Representation`] rows
/// (the text at different disclosure depths) and an optional [`MemorySource`].
/// All three are assembled together by [`build_bundle`](crate::build_bundle)
/// and persisted atomically by `Store::record_memory`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Unique identifier — `mem_<ULID>`.
    pub id: MemoryId,
    /// Owning project — enforces the per-project scope boundary.
    pub project_id: ProjectId,
    /// Semantic classification used for ranking and context assembly.
    pub r#type: MemoryType,
    /// Soft-delete lifecycle state. Never `DELETE` a row — flip this instead.
    pub status: MemoryStatus,
    /// Model confidence in the memory, in `[0.0, 1.0]`. Always `1.0` for
    /// human-authored memories; reserved for future ML-derived entries.
    pub confidence: f64,
    /// Author-supplied signal strength, in `[0.0, 1.0]`. Influences ranking
    /// via the `importance_weight` term in [`HybridOpts`](crate::HybridOpts).
    pub importance: f64,
    /// Insertion timestamp (RFC3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last-mutation timestamp — drives the recency boost in
    /// [`composite_score`](crate::composite_score).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Set when `status == Deleted`; `None` for active memories.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

/// One representation of a memory at a specific disclosure depth.
///
/// Every memory has exactly four representations (one per [`RepresentationDepth`]),
/// derived deterministically by [`representations::derive`](crate::representations).
/// The `content_hash` enables change detection when re-deriving after an edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Representation {
    /// Back-reference to the owning [`Memory`].
    pub memory_id: MemoryId,
    /// Which disclosure level this content represents.
    pub depth: RepresentationDepth,
    /// The text at this depth — length varies by `depth`.
    pub content: String,
    /// Approximate token count, populated lazily by the embedding pipeline.
    pub token_count: Option<i64>,
    /// SHA-256 (first 16 bytes, hex) of `content` — detects stale representations.
    pub content_hash: Option<String>,
}

/// Optional provenance attached to a memory at capture time.
///
/// Source content is capped at [`SOURCE_SNIPPET_MAX_BYTES`](crate::SOURCE_SNIPPET_MAX_BYTES)
/// (2 KiB) before persistence. Truncation is flagged in the
/// [`SourceRow::truncated`](crate::SourceRow) field, not here, because
/// `MemorySource` is the read-side projection from the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySource {
    /// Back-reference to the owning [`Memory`].
    pub memory_id: MemoryId,
    /// Category of the source — e.g. `"file"`, `"url"`, `"clipboard"`.
    pub source_type: String,
    /// File path, URL, or other stable locator — `None` if not applicable.
    pub source_ref: Option<String>,
    /// Verbatim snippet, capped at 2 KiB by the CLI before persistence.
    pub source_content: Option<String>,
}

/// An append-only journal entry describing a mutation to the memory store.
///
/// `memory_events` is the durable source-of-truth. `memories` and
/// `memory_representations` are derived views that can be rebuilt from it.
/// Events are never edited or deleted (PRD §9 source-of-truth separation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    /// Project scope for this event.
    pub project_id: ProjectId,
    /// Event kind — e.g. `"remember"`, `"forget"`, `"restore"`.
    pub event_type: String,
    /// Freeform JSON payload carrying the full event detail for replay.
    pub payload_json: Option<String>,
}

/// Counts of memories by status, scoped to a single project. Returned by
/// `Store::memory_counts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCounts {
    /// Number of memories with `status = 'active'`.
    pub active: i64,
    /// Number of memories with `status = 'deleted'` (soft-deleted).
    pub deleted: i64,
}
