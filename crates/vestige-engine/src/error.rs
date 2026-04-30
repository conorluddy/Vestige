//! Typed error enum for `vestige-engine`.
//!
//! `EngineError` wraps downstream errors from `vestige-store` and
//! `vestige-embed` and adds engine-specific variants. CLI/MCP callers
//! convert to `anyhow::Error` or `ErrorData` at their boundaries.
//!
//! # MCP mapping
//!
//! | Variant | MCP error code | `retryable` |
//! |---------|---------------|-------------|
//! | `Store` | `STORE_FAILED` | `true` |
//! | `Embed` | `EMBED_FAILED` | `false` |
//! | `EmbeddingsUnavailable` | `EMBEDDINGS_UNAVAILABLE` | `false` |

use thiserror::Error;

/// Errors produced by the engine search and embed pipelines.
#[derive(Debug, Error)]
pub enum EngineError {
    /// Store operation failed.
    ///
    /// Transient (disk I/O, lock timeout) — propagate `retryable = true` at
    /// the MCP boundary so agents know a retry may succeed.
    #[error("store: {0}")]
    Store(#[from] vestige_store::StoreError),

    /// Embedding provider operation failed.
    ///
    /// Covers both network errors (fastembed remote) and model-load failures.
    /// Not considered retryable at the MCP boundary — the provider config
    /// should be fixed before retrying.
    #[error("embed: {0}")]
    Embed(#[from] vestige_embed::EmbedError),

    /// Embeddings are unavailable for the requested mode.
    ///
    /// Distinct from an in-band warning: this variant is raised only when the
    /// engine cannot proceed at all — for example, `search_semantic` with zero
    /// embeddings and no fallback path. Most hybrid paths surface unavailability
    /// as a warning inside [`crate::search::HybridOutcome`] instead and fall
    /// back gracefully to lexical search.
    #[error("embeddings unavailable: {0}")]
    EmbeddingsUnavailable(String),
}

/// Convenience alias — `EngineError` as the error type.
pub type Result<T> = std::result::Result<T, EngineError>;
