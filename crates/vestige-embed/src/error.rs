//! Typed error enum for the `vestige-embed` crate.
//!
//! All variants are non-retryable by default except [`EmbedError::Network`],
//! which may succeed on a subsequent attempt if connectivity is restored.
use thiserror::Error;

/// Errors returned by [`EmbeddingProvider`](crate::EmbeddingProvider) implementations
/// and [`build_provider`](crate::factory::build_provider).
#[derive(Debug, Error)]
pub enum EmbedError {
    /// The caller passed an empty string. Every provider rejects this because
    /// an empty embedding has no meaningful interpretation.
    #[error("input text is empty")]
    EmptyInput,

    /// The requested model name is not known to the provider, or the provider
    /// failed to load/initialise it (e.g. corrupt ONNX cache).
    ///
    /// `model` — the name that was requested.
    /// `reason` — a human-readable explanation (unknown name, load error, etc.).
    #[error("model `{model}` not available: {reason}")]
    ModelNotAvailable { model: String, reason: String },

    /// A transient network error occurred — typically Ollama is unreachable.
    ///
    /// This variant is the only one that may resolve without a code change;
    /// callers should treat it as retryable.
    #[error("network error: {0}")]
    Network(String),

    /// The backend returned an unexpected response or internal error
    /// (e.g. HTTP error status, malformed JSON, empty result slice).
    #[error("backend error: {0}")]
    Backend(String),

    /// The provider name in config does not match any known backend.
    ///
    /// Valid names at compile time: `"fake"`, `"fastembed"`, `"ollama"`.
    #[error("unknown provider: `{0}` (valid: fake, fastembed, ollama)")]
    UnknownProvider(String),

    /// The named provider is recognised but was not compiled into this binary.
    ///
    /// Rebuild with `--features <name>` to enable it.  Returned by
    /// [`build_provider`](crate::factory::build_provider) when the matching
    /// feature flag is absent.
    #[error("provider `{0}` not enabled in this build (rebuild with --features {0})")]
    ProviderDisabled(&'static str),
}
