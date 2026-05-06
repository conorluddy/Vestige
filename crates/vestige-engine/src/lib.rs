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

pub mod candidate;
pub mod embed;
pub mod error;
pub mod search;

// Re-export public candidate surface so callers don't need `vestige_engine::candidate::*`.
pub use candidate::{
    approve_candidate, propose_candidate, reject_candidate, ApprovalOutcome, ApprovalOverrides,
    ProposeOutcome, SimilarCandidate, SimilarMemory,
};
