//! `vestige-engine` — orchestration layer for search and embed pipelines.
//!
//! CLI and MCP are thin adapters; this crate owns the business logic that was
//! previously duplicated across both. Depends on `vestige-core`,
//! `vestige-store`, and `vestige-embed`. Never imports `clap`, `rmcp`, or
//! `anyhow`.
//!
//! # Modules
//!
//! - [`search`] — lexical / semantic / hybrid retrieval, returning [`search::HybridOutcome`].
//! - [`embed`] — per-memory and bulk embedding ingest, returning [`embed::EmbedResult`] lists.
//! - [`error`] — typed [`error::EngineError`] and [`error::Result`].

pub mod embed;
pub mod error;
pub mod search;
