//! Typed error enum for the vestige-embed crate.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("input text is empty")]
    EmptyInput,

    #[error("model `{model}` not available: {reason}")]
    ModelNotAvailable { model: String, reason: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("unknown provider: `{0}` (valid: fake, fastembed, ollama)")]
    UnknownProvider(String),

    #[error("provider `{0}` not enabled in this build (rebuild with --features {0})")]
    ProviderDisabled(&'static str),
}
