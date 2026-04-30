//! Engine error type. Wraps `StoreError` and `EmbedError` and adds
//! engine-specific variants.

use thiserror::Error;
use vestige_embed::EmbedError;
use vestige_store::StoreError;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("store: {0}")]
    Store(#[from] StoreError),

    #[error("embed: {0}")]
    Embed(#[from] EmbedError),

    #[error("embeddings unavailable: {0}")]
    EmbeddingsUnavailable(String),

    #[error("validation: {0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
