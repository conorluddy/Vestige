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
use vestige_core::CandidateStatus;

/// Errors produced by the engine search, embed, and candidate pipelines.
#[derive(Debug, Error)]
pub enum EngineError {
    /// Store operation failed.
    ///
    /// Transient (disk I/O, lock timeout) — propagate `retryable = true` at
    /// the MCP boundary so agents know a retry may succeed.
    #[error("store: {0}")]
    Store(#[from] vestige_store::StoreError),

    /// A `vestige-core` domain operation failed (e.g. `build_bundle` validation).
    #[error("core: {0}")]
    Core(#[from] vestige_core::CoreError),

    /// Embedding provider operation failed.
    ///
    /// Covers both network errors (fastembed remote) and model-load failures.
    /// Not considered retryable at the MCP boundary — the provider config
    /// should be fixed before retrying.
    #[error("embed: {0}")]
    Embed(#[from] vestige_embed::EmbedError),

    /// Session-log ingestion (discovery / transcript read) failed.
    ///
    /// Transient I/O failures during a session scan. Per-session extraction
    /// failures are handled inside the scan (warn + skip) and do not surface here.
    #[error("ingest: {0}")]
    Ingest(#[from] crate::ingest::IngestError),

    /// Embeddings are unavailable for the requested mode.
    ///
    /// Distinct from an in-band warning: this variant is raised only when the
    /// engine cannot proceed at all — for example, `search_semantic` with zero
    /// embeddings and no fallback path. Most hybrid paths surface unavailability
    /// as a warning inside [`crate::search::HybridOutcome`] instead and fall
    /// back gracefully to lexical search.
    #[error("embeddings unavailable: {0}")]
    EmbeddingsUnavailable(String),

    /// A candidate was not found for the given ID.
    #[error("candidate not found: `{id}`")]
    CandidateNotFound {
        /// The candidate ID that was looked up.
        id: String,
    },

    /// The candidate is not in `Pending` status and cannot be transitioned.
    #[error("candidate is not pending (status = {status})")]
    CandidateNotPending {
        /// The actual status found.
        status: CandidateStatus,
    },

    /// The candidate belongs to a different project than the caller's scope.
    #[error("candidate is out of scope for this project")]
    OutOfScope,

    /// Input validation failed (e.g. `duplicate_of` set with a non-Duplicate reason).
    #[error("validation: {message}")]
    Validation {
        /// Human-readable description of what failed.
        message: String,
    },

    /// A query trace was not found for the given ID in the current project.
    #[error("trace not found: `{id}`")]
    TraceNotFound {
        /// The trace ID that was looked up.
        id: String,
    },
}

/// Convenience alias — `EngineError` as the error type.
pub type Result<T> = std::result::Result<T, EngineError>;
