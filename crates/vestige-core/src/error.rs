//! Domain error type for `vestige-core`.
//!
//! [`CoreError`] is the single typed error for all pure-domain failures.
//! Persistence errors surface through `vestige-store`'s own `StoreError`, which
//! wraps or maps to `CoreError` at crate boundaries. At the CLI boundary, callers
//! convert to `anyhow` with `.context("…")`; at the MCP boundary, errors are
//! further mapped to `{code, message, retryable}` JSON for agents.

use thiserror::Error;

/// Convenience alias — every fallible function in `vestige-core` returns this.
pub type Result<T> = std::result::Result<T, CoreError>;

/// All domain-level failures in `vestige-core`.
///
/// Variants carry the offending value as a human-readable `String` so callers
/// can surface actionable messages without an additional lookup. Use
/// [`CoreError::Validation`] for business-rule violations that originate from
/// user input; [`CoreError::Storage`] for low-level I/O errors that bubble up
/// from `vestige-store` but need to be expressed at the core boundary.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A [`MemoryId`](crate::MemoryId) was provided but the corresponding
    /// memory row does not exist (or was never persisted).
    #[error("memory not found: {0}")]
    MemoryNotFound(String),

    /// A [`ProjectId`](crate::ProjectId) was provided but the corresponding
    /// project row does not exist.
    #[error("project not found: {0}")]
    ProjectNotFound(String),

    /// A string failed the prefix check in [`MemoryId::from_str`],
    /// [`ProjectId::from_str`], or [`EmbeddingId::from_str`](crate::EmbeddingId).
    #[error("invalid id: {0}")]
    InvalidId(String),

    /// A raw string could not be parsed as a [`MemoryType`](crate::MemoryType).
    #[error("invalid memory type: {0}")]
    InvalidMemoryType(String),

    /// A raw string could not be parsed as a [`MemoryStatus`](crate::MemoryStatus).
    #[error("invalid memory status: {0}")]
    InvalidMemoryStatus(String),

    /// A raw string could not be parsed as a [`RepresentationDepth`](crate::RepresentationDepth).
    #[error("invalid representation depth: {0}")]
    InvalidDepth(String),

    /// A business-rule was violated — e.g. empty body, out-of-range importance,
    /// or an unrecognised search mode. Message should name the field and expected range.
    #[error("validation: {0}")]
    Validation(String),

    /// A low-level I/O or SQL error that crossed from `vestige-store` into
    /// `vestige-core`. Retryable if the underlying operation is idempotent.
    #[error("storage: {0}")]
    Storage(String),
}
