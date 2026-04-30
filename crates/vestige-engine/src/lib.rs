//! Vestige memory engine — orchestration over `Store` + `EmbeddingProvider`.
//!
//! Hosts hybrid search and embed-ingest functions that are too high-level for
//! `vestige-core` (which forbids store/embed deps) and too duplicated to leave
//! in CLI/MCP. Both adapters call into here.

pub mod embed;
pub mod error;
pub mod search;

pub use error::{EngineError, Result};
