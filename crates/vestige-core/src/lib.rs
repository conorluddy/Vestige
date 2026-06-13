//! Pure domain core for Vestige — types, typed IDs, errors, representation
//! derivation, search primitives, and context-pack assembly.
//!
//! No `rusqlite`, no `clap`, no `rmcp`, no `async`. Callers in `vestige-store`,
//! `vestige-cli`, and `vestige-mcp` depend on this crate; it never depends on
//! them. See the workspace `CLAUDE.md` for the one-way dependency graph.

pub mod candidate;
pub mod context;
pub mod error;
pub mod ids;
pub mod memory;
pub mod redaction;
pub mod representations;
pub mod types;

// ========================================
// === ERRORS & IDS ===
// ========================================
pub use error::{CoreError, Result};
pub use ids::{CandidateId, EmbeddingId, MemoryId, ProjectId, TraceId};

// ========================================
// === DOMAIN TYPES ===
// ========================================
pub use types::{
    CandidateStatus, Memory, MemoryCounts, MemoryEvent, MemorySource, MemoryStatus, MemoryType,
    ProjectRecord, Representation, RepresentationDepth, SourceKind,
};

// ========================================
// === CAPTURE & PROJECTION ===
// ========================================
pub use memory::{
    build_bundle, pick_representation, project_card, project_detail, truncate_at_utf8_boundary,
    FetchedMemory, MemoryBundle, MemoryCard, MemoryDetail, NewMemory, NewSource, RepresentationRow,
    ScoredCard, SourceRow, SOURCE_SNIPPET_MAX_BYTES,
};

// ========================================
// === SEARCH & RANKING ===
// ========================================
pub use memory::{
    composite_score, merge_hits, normalise_cosine, normalise_fts, rank_hits, resolve_default_mode,
    sanitize_fts_query, HybridOpts, HybridScore, ListFilter, SearchFilter, SearchHit, SearchMode,
    SemanticHit,
};

// ========================================
// === CANDIDATE INBOX (V0.2) ===
// ========================================
pub use candidate::{
    build_candidate_bundle, Candidate, CandidateBundle, CandidateSource, NewCandidate,
    NewCandidateSource, RejectionReason,
};

// ========================================
// === CONTEXT PACKS ===
// ========================================
pub use context::{
    build_pack, ContextOptions, ContextPack, ContextSections, ContextSources,
    APPROX_CHARS_PER_TOKEN,
};

// ========================================
// === REDACTION ===
// ========================================
pub use redaction::redact_secrets;
