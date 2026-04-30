//! Memory engine — pure functions that build persistable bundles from user
//! input and project bundles back into agent-friendly cards / details.
//!
//! All persistence and SQL lives in `vestige-store`. This module owns the
//! shape of the data and the derivation rules.

mod bundle;
mod projection;
mod scoring;
mod search;

pub use bundle::{
    build_bundle, truncate_at_utf8_boundary, MemoryBundle, NewMemory, NewSource, RepresentationRow,
    SourceRow, SOURCE_SNIPPET_MAX_BYTES,
};
pub use projection::{
    pick_representation, project_card, project_detail, FetchedMemory, MemoryCard, MemoryDetail,
};
pub use scoring::{
    composite_score, merge_hits, normalise_cosine, normalise_fts, rank_hits, HybridScore,
    ScoredCard,
};
pub use search::{
    resolve_default_mode, sanitize_fts_query, HybridOpts, ListFilter, SearchFilter, SearchHit,
    SearchMode, SemanticHit,
};
