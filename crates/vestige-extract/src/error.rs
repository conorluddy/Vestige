//! Typed error enum for the `vestige-extract` crate.
//!
//! Mirrors [`vestige_embed::EmbedError`] in shape: all variants are non-retryable
//! by default except [`ExtractError::Network`], which may succeed on a subsequent
//! attempt if connectivity to the model backend is restored.
use thiserror::Error;

/// Errors returned by [`ExtractionProvider`](crate::ExtractionProvider) implementations
/// and [`build_provider`](crate::factory::build_provider).
#[derive(Debug, Error)]
pub enum ExtractError {
    /// The caller passed an empty batch of turns. There is nothing to extract.
    #[error("no turns to extract")]
    EmptyInput,

    /// A required credential (API key) was not found in the environment.
    ///
    /// `0` — the name of the missing environment variable.
    #[error("missing credential: environment variable `{0}` is not set")]
    MissingCredential(&'static str),

    /// A transient network error occurred — typically the model backend is unreachable.
    ///
    /// This variant is the only one that may resolve without a code change;
    /// callers should treat it as retryable.
    #[error("network error: {0}")]
    Network(String),

    /// The backend returned an unexpected response or internal error
    /// (e.g. HTTP error status, malformed JSON, no parseable candidates).
    #[error("backend error: {0}")]
    Backend(String),

    /// The provider name in config does not match any known backend.
    ///
    /// Valid names at compile time: `"fake"`, `"ollama"`, `"anthropic"`, `"openai"`.
    #[error("unknown provider: `{0}` (valid: fake, ollama, anthropic, openai)")]
    UnknownProvider(String),

    /// The named provider is recognised but was not compiled into this binary.
    ///
    /// Rebuild with `--features <name>` to enable it. Returned by
    /// [`build_provider`](crate::factory::build_provider) when the matching
    /// feature flag is absent.
    #[error("provider `{0}` not enabled in this build (rebuild with --features {0})")]
    ProviderDisabled(&'static str),
}
