//! LLM extraction provider abstraction for Vestige V0.5.4 (daemon-mode session ingestion).
//!
//! Providers read a batch of [`vestige_core::NormalizedTurn`]s and propose durable memories
//! via the [`ExtractionProvider`] trait. Select a backend by calling [`build_provider`] with
//! an [`ExtractionConfig`] — typically deserialised from the `[extraction]` block of
//! `.vestige/config.toml`.
//!
//! This crate is the extraction analogue of `vestige-embed`: a sync trait, a deterministic
//! `fake` backend that is always compiled, and feature-gated real backends. The agent-driven
//! ingestion path (V0.5.3, the `vestige_scan_sessions` MCP tool) never touches this crate —
//! only the daemon's `session_log_scan` job and the one-shot `vestige scan` CLI do.
//!
//! # Providers
//!
//! | Backend     | Feature flag  | Notes                                            |
//! |-------------|---------------|--------------------------------------------------|
//! | `fake`      | *(always on)* | Deterministic; tests only.                       |
//! | `ollama`    | `ollama`      | Local Ollama daemon. Default real provider.      |
//! | `anthropic` | `anthropic`   | Claude Messages API; needs `ANTHROPIC_API_KEY`.  |
//! | `openai`    | `openai`      | Chat Completions API; needs `OPENAI_API_KEY`.    |
//!
//! Dependency edge: `vestige-engine → vestige-extract → vestige-core`. This crate imports no
//! `rusqlite`, no `clap`, no `rmcp`, and no sibling crate other than `vestige-core`.

pub mod error;
pub mod factory;
pub mod fake;
pub mod prompt;
pub mod provider;

/// Ollama provider (requires `--features ollama`).
#[cfg(feature = "ollama")]
pub mod ollama;
/// Re-export of [`ollama::OllamaExtractionProvider`] (requires `--features ollama`).
#[cfg(feature = "ollama")]
pub use ollama::OllamaExtractionProvider;

/// Anthropic provider (requires `--features anthropic`).
#[cfg(feature = "anthropic")]
pub mod anthropic;
/// Re-export of [`anthropic::AnthropicExtractionProvider`] (requires `--features anthropic`).
#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicExtractionProvider;

/// OpenAI provider (requires `--features openai`).
#[cfg(feature = "openai")]
pub mod openai;
/// Re-export of [`openai::OpenAiExtractionProvider`] (requires `--features openai`).
#[cfg(feature = "openai")]
pub use openai::OpenAiExtractionProvider;

/// Typed error enum for all extraction operations in this crate.
pub use error::ExtractError;
/// Provider factory and its configuration type.
pub use factory::{build_provider, ExtractionConfig};
/// Deterministic provider for tests (always compiled in).
pub use fake::FakeExtractionProvider;
/// Core trait every extraction backend implements, plus its output type.
pub use provider::{ExtractedCandidate, ExtractionProvider};
