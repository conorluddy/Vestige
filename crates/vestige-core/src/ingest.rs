//! Shared ingestion primitives — the source-agnostic turn type.
//!
//! [`NormalizedTurn`] is the common shape every transcript adapter normalises its
//! source format into. It lives in `vestige-core` (rather than `vestige-engine`)
//! so that both the engine's `ingest` source layer **and** the `vestige-extract`
//! crate can depend on it without crossing the one-way crate boundary
//! (`engine → extract → core`).

// === PUBLIC TYPES ===

/// A normalised conversational turn extracted from a coding-agent transcript.
///
/// All adapters normalise their source format into this common shape so downstream
/// processing (candidate proposal, redaction, deduplication, LLM extraction) is
/// source-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTurn {
    /// Speaker role: `"user"`, `"assistant"`, `"system"`, or a source-specific value.
    pub role: String,
    /// Plain-text content of the turn. May be empty for turns with only tool-call payloads.
    pub text: String,
    /// 1-based line index within the source `.jsonl` file (for provenance attribution).
    pub line: usize,
}
