//! `vestige-engine` — orchestration layer for search and embed pipelines.
//!
//! `vestige-core` is deliberately free of storage and embedding concerns so
//! that its domain types stay portable and testable without SQLite or a
//! network provider. This crate bridges that gap: it combines `vestige-core`
//! ranking primitives, `vestige-store` persistence, and `vestige-embed`
//! provider traits into end-to-end pipelines that both `vestige-cli` and
//! `vestige-mcp` call without duplicating the logic.
//!
//! # Crate boundary rule
//!
//! `vestige-engine` must **not** import `clap`, `rmcp`, or `anyhow`. It
//! surfaces typed [`error::EngineError`] values; callers convert to their own
//! boundary types (CLI → `anyhow`, MCP → `ErrorData`).
//!
//! # Modules
//!
//! - [`search`] — lexical / semantic / hybrid retrieval, single source of
//!   truth for all three modes; returns [`search::HybridOutcome`].
//! - [`embed`] — per-memory and bulk embedding ingest with idempotent
//!   skip-if-current logic; returns [`embed::EmbedResult`] lists.
//! - [`error`] — typed [`error::EngineError`] and [`error::Result`].
//! - [`trace`] — engine tracing hook; single write site for `query_events`.

pub mod candidate;
pub mod context;
pub mod embed;
pub mod error;
pub mod provenance;
pub mod replay;
pub mod search;
pub mod trace;
pub mod trace_read;

// Re-export public candidate surface so callers don't need `vestige_engine::candidate::*`.
pub use candidate::{
    approve_candidate, propose_candidate, reject_candidate, ApprovalOutcome, ApprovalOverrides,
    ProposeOutcome, SimilarCandidate, SimilarMemory,
};

// Re-export Caller so CLI/MCP can import it without knowing the module path.
pub use trace::Caller;

// Re-export provenance surface so callers don't need `vestige_engine::provenance::*`.
pub use provenance::{
    list_sources, walk_provenance, CandidateProvenance, EventEntry, MemoryProvenance,
    ProvenanceWalk, SourceListing, SourceReceipt, SubjectId,
};

// Re-export trace read surface so CLI and MCP don't need the module path.
pub use trace_read::{get_trace, list_traces, ListFilters, TraceCard, TraceDetail};

// Re-export replay surface so CLI and MCP don't need the module path.
pub use replay::{replay_trace, ReplayDiff, ReplayResult, ReplayResultSet, ScoreChange};
