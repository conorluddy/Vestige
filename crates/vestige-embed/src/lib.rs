//! Embedding provider abstraction for Vestige V0.1.
//!
//! Providers convert text into fixed-dimension `Vec<f32>` vectors via the
//! [`EmbeddingProvider`] trait. Select a backend by calling [`build_provider`]
//! with an [`EmbeddingsConfig`] — typically deserialised from
//! `.vestige/config.toml`.
//!
//! # Providers
//!
//! | Backend           | Feature flag  | Notes                                      |
//! |-------------------|---------------|--------------------------------------------|
//! | `fake`            | *(always on)* | Deterministic SHA-256 hashes; tests only.  |
//! | `fastembed`       | `fastembed`   | Local ONNX models; ~60 MB download once.   |
//! | `ollama`          | `ollama`      | Requires a running Ollama daemon.          |
//!
//! The `fake` provider is always compiled in so the test suite never needs
//! network access or model downloads.

pub mod error;
pub mod factory;
pub mod fake;
pub mod provider;

/// FastEmbed ONNX provider (requires `--features fastembed`).
#[cfg(feature = "fastembed")]
pub mod fastembed;
/// Re-export of [`fastembed::FastembedProvider`] (requires `--features fastembed`).
#[cfg(feature = "fastembed")]
pub use fastembed::FastembedProvider;

/// Ollama provider (requires `--features ollama`).
#[cfg(feature = "ollama")]
pub mod ollama;
/// Re-export of [`ollama::OllamaProvider`] (requires `--features ollama`).
#[cfg(feature = "ollama")]
pub use ollama::OllamaProvider;

/// Typed error enum for all embedding operations in this crate.
pub use error::EmbedError;
/// Provider factory and its configuration type.
pub use factory::{build_provider, EmbeddingsConfig};
/// Deterministic hash-based provider for tests (always compiled in).
pub use fake::FakeEmbeddingProvider;
/// Core trait every embedding backend implements.
pub use provider::EmbeddingProvider;
