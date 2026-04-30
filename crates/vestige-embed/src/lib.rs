//! Embedding provider abstraction for Vestige V0.1.
//!
//! Providers convert text into deterministic-shape `Vec<f32>` vectors. The
//! `FakeEmbeddingProvider` is always available (no feature flag) so the test
//! suite never needs network or model downloads. Real providers
//! (`fastembed`, `ollama`) land in PR7 behind cargo features.

pub mod error;
pub mod factory;
pub mod fake;
pub mod provider;

#[cfg(feature = "fastembed")]
pub mod fastembed;
#[cfg(feature = "fastembed")]
pub use fastembed::FastembedProvider;

#[cfg(feature = "ollama")]
pub mod ollama;
#[cfg(feature = "ollama")]
pub use ollama::OllamaProvider;

pub use error::EmbedError;
pub use factory::{build_provider, EmbeddingsConfig};
pub use fake::FakeEmbeddingProvider;
pub use provider::EmbeddingProvider;
