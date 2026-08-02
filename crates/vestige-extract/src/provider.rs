//! Core [`ExtractionProvider`] trait — the contract every extraction backend must satisfy.
//!
//! New providers implement this trait; [`build_provider`](crate::factory::build_provider)
//! selects the right implementation at runtime based on
//! [`ExtractionConfig`](crate::ExtractionConfig).
//!
//! An extraction provider reads a batch of [`NormalizedTurn`]s (a slice of a coding-agent
//! transcript) and proposes zero or more durable memories worth keeping. Unlike the
//! embedding provider — which turns text into a vector — this provider turns a
//! conversation into structured candidate proposals via an LLM.
use vestige_core::{MemoryType, NormalizedTurn};

use crate::error::ExtractError;

/// A single memory worth keeping, proposed by an [`ExtractionProvider`] from a batch of turns.
///
/// The daemon's `session_log_scan` job maps each [`ExtractedCandidate`] into a
/// `vestige_core::NewCandidate` and routes it through the existing
/// `propose_candidate` path (the V0.2 inbox) — it is **never** auto-promoted.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedCandidate {
    /// Semantic classification for the proposed memory.
    pub proposed_type: MemoryType,
    /// The candidate body — the durable fact, decision, or preference worth recording.
    pub body: String,
    /// Why the extractor believes this is worth keeping. `None` when the model gave none.
    pub rationale: Option<String>,
    /// Model confidence in `[0.0, 1.0]`. Callers clamp to the valid range before storage.
    pub confidence: f32,
}

/// Reads a batch of transcript turns and proposes durable memories worth keeping.
///
/// Implementations must be `Send + Sync` so a single instance can be shared across the
/// daemon's per-project workers. Methods are plain synchronous `fn` (mirroring
/// [`EmbeddingProvider`](vestige_embed::EmbeddingProvider)); backends that need I/O use a
/// blocking client internally rather than forcing `async` on every caller.
///
/// The two introspection methods — [`provider_name`] and [`model_name`] — are recorded in
/// the daemon's logs and (future) provenance so a run can be attributed to a specific model.
///
/// [`provider_name`]: ExtractionProvider::provider_name
/// [`model_name`]: ExtractionProvider::model_name
pub trait ExtractionProvider: Send + Sync {
    /// Short, stable identifier for the provider backend
    /// (e.g. `"fake"`, `"ollama"`, `"anthropic"`, `"openai"`).
    fn provider_name(&self) -> &'static str;

    /// Name of the specific model in use (e.g. `"deterministic"`, `"llama3.2"`, `"claude-..."`).
    fn model_name(&self) -> &str;

    /// Extract zero or more candidate memories from a batch of normalised turns.
    ///
    /// An empty return is valid and expected — most conversation slices contain nothing
    /// worth keeping. Implementations must **never** dump raw turns as candidates when the
    /// backend is unavailable; they return [`ExtractError`] so the caller can no-op.
    ///
    /// # Errors
    ///
    /// - [`ExtractError::EmptyInput`] — `turns` is empty.
    /// - [`ExtractError::MissingCredential`] — a required API key is unset.
    /// - [`ExtractError::Network`] — a network call to the model backend failed.
    /// - [`ExtractError::Backend`] — the backend returned an unexpected/malformed response.
    fn extract(&self, turns: &[NormalizedTurn]) -> Result<Vec<ExtractedCandidate>, ExtractError>;
}
