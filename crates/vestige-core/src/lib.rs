pub mod context;
pub mod error;
pub mod ids;
pub mod memory;
pub mod representations;
pub mod types;

pub use context::{
    build_pack, ContextOptions, ContextPack, ContextSections, ContextSources,
    APPROX_CHARS_PER_TOKEN,
};
pub use error::{CoreError, Result};
pub use ids::{EmbeddingId, MemoryId, ProjectId};
pub use memory::{
    build_bundle, composite_score, merge_hits, normalise_cosine, normalise_fts, project_card,
    project_detail, rank_hits, resolve_default_mode, sanitize_fts_query, truncate_at_utf8_boundary,
    FetchedMemory, HybridOpts, HybridScore, ListFilter, MemoryBundle, MemoryCard, MemoryDetail,
    NewMemory, NewSource, RepresentationRow, ScoredCard, SearchFilter, SearchHit, SearchMode,
    SemanticHit, SourceRow, SOURCE_SNIPPET_MAX_BYTES,
};
pub use types::{
    Memory, MemoryEvent, MemorySource, MemoryStatus, MemoryType, ProjectRecord, Representation,
    RepresentationDepth,
};
