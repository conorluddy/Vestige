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

pub use error::EmbedError;
pub use factory::{build_provider, EmbeddingsConfig};
pub use fake::FakeEmbeddingProvider;
pub use provider::EmbeddingProvider;
