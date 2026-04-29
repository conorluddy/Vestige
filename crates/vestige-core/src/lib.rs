pub mod error;
pub mod ids;
pub mod memory;
pub mod representations;
pub mod types;

pub use error::{CoreError, Result};
pub use ids::{MemoryId, ProjectId};
pub use memory::{
    build_bundle, composite_score, project_card, project_detail, rank_hits, sanitize_fts_query,
    FetchedMemory, ListFilter, MemoryBundle, MemoryCard, MemoryDetail, NewMemory, NewSource,
    RepresentationRow, ScoredCard, SearchFilter, SearchHit, SourceRow, SOURCE_SNIPPET_MAX_BYTES,
};
pub use types::{
    Memory, MemoryEvent, MemorySource, MemoryStatus, MemoryType, ProjectRecord, Representation,
    RepresentationDepth,
};
