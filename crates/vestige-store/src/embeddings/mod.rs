//! Vector storage and nearest-neighbour retrieval for Vestige V0.1.
//!
//! # Strategy
//!
//! `sqlite-vec` was evaluated but abandoned: `sqlite3_vec_init` carries no
//! arguments while `rusqlite::auto_extension::RawAutoExtension` requires the
//! standard 3-arg SQLite entrypoint signature. Wiring them together requires
//! an `unsafe transmute` with mismatched ABI — exactly the "unsafe extern
//! typing mismatch" flag in the brief.
//!
//! Brute-force cosine scan over the `memory_vectors` BLOB column is the
//! fallback: read all active, in-project, matching-provider rows; decode
//! little-endian `f32[]`; rank in Rust. At V0.1 data volumes (<10 k vectors)
//! this is fast enough. A future PR can add a `vec0` virtual table behind
//! the same `Store` API once the sqlite-vec integration stabilises.
//!
//! # File layout
//!
//! - `mod.rs` — types + private helpers shared across submodules + tests.
//! - `record.rs` — record/stale/delete write path.
//! - `nearest.rs` — brute-force cosine scan.
//! - `status.rs` — `embedding_status` coverage snapshot.
//! - `jobs.rs` — `Store` impl methods around the `embedding_jobs` table and
//!   per-representation lookup helpers used by `vestige-engine`.

mod jobs;
mod nearest;
mod record;
mod status;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::{EmbeddingId, MemoryId, MemoryType, ProjectId};

use crate::{Result, StoreError};

pub(crate) use nearest::nearest_neighbours;
pub(crate) use record::{
    delete_embedding, mark_embedding_stale, mark_representation_embeddings_stale, record_embedding,
};
pub(crate) use status::embedding_status;

// === TYPES ===

/// Input for recording a new (or replacement) embedding.
///
/// `INSERT OR REPLACE` semantics mean that calling this twice for the same
/// `(representation_id, provider, model)` triple silently replaces the old row.
pub struct NewEmbedding<'a> {
    pub memory_id: &'a MemoryId,
    pub representation_id: &'a str,
    pub representation_type: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub vector: &'a [f32],
}

/// Filters applied when querying for nearest neighbours.
pub struct VectorFilter {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    /// When `Some`, only memories of this type are returned.
    pub memory_type: Option<MemoryType>,
}

/// One result from [`crate::Store::nearest_neighbours`].
pub struct VectorHit {
    pub memory_id: MemoryId,
    pub embedding_id: EmbeddingId,
    pub representation_id: String,
    pub representation_type: String,
    /// Cosine similarity in [-1, 1]. Typically [0, 1] for L2-normalised vectors.
    pub similarity: f64,
}

/// Snapshot of embedding coverage for a project. Used by `vestige embeddings status`.
pub struct EmbeddingStatus {
    pub project_id: ProjectId,
    /// Dominant provider (by count of active embeddings), if any exist.
    pub provider: Option<String>,
    /// Dominant model, if any active embeddings exist.
    pub model: Option<String>,
    /// Dimension count for the dominant model, if known.
    pub dimensions: Option<usize>,
    /// Total active memories in the project.
    pub total_active_memories: u64,
    /// Embeddable representation count: `summary` + `compressed_body` for active memories.
    pub embeddable_representations: u64,
    /// Representations that have an active embedding.
    pub embedded_representations: u64,
    /// Embeddings currently marked stale.
    pub stale_embeddings: u64,
    /// Embedding job rows with status `'failed'`.
    pub failed_jobs: u64,
    /// Embeddable representations with no active (or stale) embedding.
    ///
    /// Computed as `embeddable - embedded - stale`, saturating at 0.
    pub missing_embeddings: u64,
}

// === PRIVATE HELPERS (shared across submodules) ===

/// Encode a `&[f32]` as little-endian bytes for BLOB storage.
pub(super) fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for &v in vector {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Decode a BLOB back into `Vec<f32>`.
///
/// Returns [`StoreError::Corruption`] if the BLOB length is not a multiple of 4.
/// A partial trailing f32 means either a writer bug or on-disk damage — both
/// warrant loud failure rather than a silently-truncated vector.
pub(super) fn decode_vector(blob: &[u8]) -> Result<Vec<f32>> {
    if blob.len() % 4 != 0 {
        return Err(StoreError::Corruption(format!(
            "vector blob length {} is not a multiple of 4",
            blob.len()
        )));
    }
    Ok(blob
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

/// Cosine similarity between two float slices.
///
/// Returns `None` when the result would be undefined: dimension mismatch or
/// either vector has zero norm. Callers skip such rows rather than ranking
/// them as -1.0 (which then gets clamped to 0.0 upstream and surfaces as a
/// non-match in the result list).
pub(super) fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (ai, bi) in a.iter().zip(b.iter()) {
        let ai = *ai as f64;
        let bi = *bi as f64;
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return None;
    }
    Some(dot / denom)
}

/// Compute hex SHA-256 of the little-endian encoding of a vector.
///
/// Stored in `memory_embeddings.vector_hash` for V0.2's "vector unchanged
/// despite content rehash" optimisation: when a representation's content_hash
/// changes but a re-embed produces the same vector, the row can flip back to
/// active without a vec rewrite. Currently written, not yet read.
pub(super) fn compute_vector_hash(vector: &[f32]) -> String {
    let bytes = encode_vector(vector);
    let digest = Sha256::digest(&bytes);
    hex::encode(digest)
}

/// Generate an RFC-3339 timestamp string for `now`.
pub(super) fn rfc3339_now() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(StoreError::Time)
}

/// Map an ID parse error into a `StoreError::Sqlite` (consistent with other helpers).
pub(super) fn invalid_id_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}

/// Generate a `job_<ULID>` prefixed ID for an `embedding_jobs` row.
#[allow(dead_code)] // reserved for future job lifecycle helpers
pub(crate) fn new_job_id() -> String {
    format!("job_{}", Ulid::new())
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_vector_rejects_partial_trailing_bytes() {
        // 7 bytes — would silently truncate to one f32 under the old impl.
        let blob = vec![0u8; 7];
        let err = decode_vector(&blob).unwrap_err();
        assert!(matches!(err, StoreError::Corruption(_)));
    }

    #[test]
    fn decode_vector_round_trips_clean_blob() {
        let v = vec![1.0_f32, -2.5, 3.25];
        let blob = encode_vector(&v);
        let back = decode_vector(&blob).expect("clean blob decodes");
        assert_eq!(v, back);
    }

    #[test]
    fn cosine_similarity_returns_none_on_zero_norm() {
        assert!(cosine_similarity(&[0.0, 0.0, 0.0], &[1.0, 0.0, 0.0]).is_none());
        assert!(cosine_similarity(&[1.0, 0.0, 0.0], &[0.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn cosine_similarity_returns_none_on_dimension_mismatch() {
        assert!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn cosine_similarity_returns_none_on_empty() {
        assert!(cosine_similarity(&[], &[]).is_none());
    }

    #[test]
    fn cosine_similarity_orthogonal_is_zero() {
        let s = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(s.abs() < 1e-9);
    }

    #[test]
    fn cosine_similarity_identical_is_one() {
        let s = cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
        assert!((s - 1.0).abs() < 1e-9);
    }
}
