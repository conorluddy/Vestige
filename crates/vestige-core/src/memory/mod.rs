//! Memory engine — pure functions for capture, projection, search, and ranking.
//!
//! Sub-modules:
//! - `bundle` — assembles [`MemoryBundle`]s from [`NewMemory`] input.
//! - `projection` — converts raw store rows into [`MemoryCard`] / [`MemoryDetail`].
//! - `search` — query types, FTS sanitisation, and mode resolution.
//! - `scoring` — composite score, hybrid merge, and normalisation helpers.
//!
//! All persistence and SQL live in `vestige-store`; nothing here touches I/O.

mod bundle;
mod projection;
mod scoring;
mod search;

pub use bundle::{
    build_bundle, hash, truncate_at_utf8_boundary, MemoryBundle, NewMemory, NewSource,
    RepresentationRow, SourceRow, SOURCE_SNIPPET_MAX_BYTES,
};
pub use projection::{
    pick_representation, project_card, project_detail, FetchedMemory, MemoryCard, MemoryDetail,
};
pub use scoring::{
    composite_score, composite_score_with, merge_hits, normalise_cosine, normalise_fts, rank_hits,
    rank_hits_with, usage_boost, HybridScore, ScoredCard, UsageWeighting,
};
pub use search::{
    resolve_default_mode, sanitize_fts_query, HybridOpts, ListFilter, SearchFilter, SearchHit,
    SearchMode, SemanticHit,
};
