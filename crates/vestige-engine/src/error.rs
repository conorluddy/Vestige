//! Typed error enum for `vestige-engine`.
//!
//! `EngineError` wraps downstream errors from `vestige-store` and
//! `vestige-embed` and adds engine-specific variants. CLI/MCP callers
//! convert to `anyhow::Error` or `ErrorData` at their boundaries.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    /// Store operation failed (propagate `retryable = true` at MCP boundary).
    #[error("store: {0}")]
    Store(#[from] vestige_store::StoreError),

    /// Embedding provider operation failed.
    #[error("embed: {0}")]
    Embed(#[from] vestige_embed::EmbedError),

    /// Embeddings are unavailable for the requested mode.
    ///
    /// Distinct from an in-band warning: this variant is raised only when the
    /// engine cannot proceed at all (e.g. `search_semantic` with zero embeddings
    /// and no fallback path). Most paths surface unavailability as a warning
    /// inside [`crate::search::HybridOutcome`] instead.
    #[error("embeddings unavailable: {0}")]
    EmbeddingsUnavailable(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
