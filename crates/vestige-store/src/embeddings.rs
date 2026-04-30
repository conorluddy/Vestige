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

use sha2::{Digest, Sha256};
use std::str::FromStr;
use time::OffsetDateTime;
use tracing::warn;
use ulid::Ulid;

use vestige_core::{EmbeddingId, MemoryId, MemoryType, ProjectId, RepresentationDepth};

use crate::{Result, Store, StoreError};

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

// === PUBLIC API ===

/// Insert or replace an embedding + its vector blob in a single transaction.
///
/// Uses `INSERT OR REPLACE` on `memory_embeddings` (unique index
/// `(representation_id, provider, model)`) then inserts/replaces the
/// corresponding row in `memory_vectors`. The FK `ON DELETE CASCADE` on
/// `memory_vectors` automatically clears the old vector when the
/// `memory_embeddings` row is replaced.
///
/// Returns the `EmbeddingId` of the new row.
pub(crate) fn record_embedding(
    conn: &rusqlite::Connection,
    new: &NewEmbedding<'_>,
) -> Result<EmbeddingId> {
    let embedding_id = EmbeddingId::new();
    let dimensions = new.vector.len();
    let vector_hash = compute_vector_hash(new.vector);
    let now_str = rfc3339_now()?;

    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "INSERT OR REPLACE INTO memory_embeddings
            (id, memory_id, representation_id, representation_type,
             provider, model, dimensions, vector_hash,
             status, created_at, updated_at, stale_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, ?9, NULL)",
        rusqlite::params![
            embedding_id.as_str(),
            new.memory_id.as_str(),
            new.representation_id,
            new.representation_type,
            new.provider,
            new.model,
            dimensions as i64,
            vector_hash,
            now_str,
        ],
    )?;

    let vector_blob = encode_vector(new.vector);
    tx.execute(
        "INSERT OR REPLACE INTO memory_vectors (embedding_id, dimensions, vector)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![embedding_id.as_str(), dimensions as i64, vector_blob],
    )?;

    tx.commit()?;
    Ok(embedding_id)
}

/// Mark a single embedding stale by ID.
pub(crate) fn mark_embedding_stale(
    conn: &rusqlite::Connection,
    embedding_id: &EmbeddingId,
) -> Result<()> {
    let now_str = rfc3339_now()?;
    conn.execute(
        "UPDATE memory_embeddings
         SET status = 'stale', stale_at = ?2, updated_at = ?2
         WHERE id = ?1 AND status <> 'stale'",
        rusqlite::params![embedding_id.as_str(), now_str],
    )?;
    Ok(())
}

/// Mark all active embeddings for a representation stale. Returns rows affected.
pub(crate) fn mark_representation_embeddings_stale(
    conn: &rusqlite::Connection,
    representation_id: &str,
) -> Result<usize> {
    let now_str = rfc3339_now()?;
    let affected = conn.execute(
        "UPDATE memory_embeddings
         SET status = 'stale', stale_at = ?2, updated_at = ?2
         WHERE representation_id = ?1 AND status <> 'stale'",
        rusqlite::params![representation_id, now_str],
    )?;
    Ok(affected)
}

/// Hard-delete a single embedding row (and its vector via FK cascade).
///
/// Embeddings are a disposable acceleration layer — hard delete is acceptable
/// here (unlike memories, which are soft-deleted only).
///
/// Returns `true` if a row was deleted.
pub(crate) fn delete_embedding(
    conn: &rusqlite::Connection,
    embedding_id: &EmbeddingId,
) -> Result<bool> {
    let deleted = conn.execute(
        "DELETE FROM memory_embeddings WHERE id = ?1",
        rusqlite::params![embedding_id.as_str()],
    )?;
    Ok(deleted > 0)
}

/// Brute-force cosine nearest-neighbour scan over the project's active embeddings.
///
/// Loads all candidate vectors into Rust, computes cosine similarity, sorts
/// descending, and returns the top `k`. Acceptable for V0.1's sub-10k vectors
/// per project; a future PR can layer a `vec0` virtual table on top.
///
/// Project scope is enforced via `JOIN memories ON memory_id WHERE project_id = ?1`.
pub(crate) fn nearest_neighbours(
    conn: &rusqlite::Connection,
    project_id: &ProjectId,
    query_vec: &[f32],
    k: u32,
    filter: &VectorFilter,
) -> Result<Vec<VectorHit>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.memory_id, e.representation_id, e.representation_type, v.vector, m.type
         FROM memory_embeddings e
         JOIN memory_vectors v ON v.embedding_id = e.id
         JOIN memories m ON m.id = e.memory_id
         WHERE m.project_id = ?1
           AND m.status = 'active'
           AND e.status = 'active'
           AND e.provider = ?2
           AND e.model = ?3
           AND e.dimensions = ?4",
    )?;

    let rows: Vec<(String, String, String, String, Vec<u8>, String)> = stmt
        .query_map(
            rusqlite::params![
                project_id.as_str(),
                filter.provider,
                filter.model,
                filter.dimensions as i64,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )?
        .collect::<std::result::Result<_, _>>()?;

    let mut scored: Vec<(f64, EmbeddingId, MemoryId, String, String)> = Vec::new();

    for (emb_id_str, mem_id_str, repr_id, repr_type, blob, type_str) in rows {
        // Apply optional memory-type filter (client-side, no extra SQL param needed).
        // Skip rows whose stored type doesn't parse — coercing to Note would
        // silently let unknown types pass or fail the filter inconsistently.
        if let Some(ref required_type) = filter.memory_type {
            let row_type = match MemoryType::from_str(&type_str) {
                Ok(t) => t,
                Err(_) => {
                    warn!(
                        memory_id = %mem_id_str,
                        memory_type = %type_str,
                        "skipping row with unknown memory type during semantic search"
                    );
                    continue;
                }
            };
            if row_type != *required_type {
                continue;
            }
        }

        let candidate = decode_vector(&blob)?;
        let similarity = match cosine_similarity(query_vec, &candidate) {
            Some(s) => s,
            None => {
                warn!(
                    memory_id = %mem_id_str,
                    query_dims = query_vec.len(),
                    candidate_dims = candidate.len(),
                    "skipping row: zero-norm vector or dimension mismatch"
                );
                continue;
            }
        };

        let embedding_id = EmbeddingId::from_str(&emb_id_str).map_err(invalid_id_err)?;
        let memory_id = MemoryId::from_str(&mem_id_str).map_err(invalid_id_err)?;
        scored.push((similarity, embedding_id, memory_id, repr_id, repr_type));
    }

    // Sort descending by similarity, take top k.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k as usize);

    Ok(scored
        .into_iter()
        .map(
            |(similarity, embedding_id, memory_id, representation_id, representation_type)| {
                VectorHit {
                    memory_id,
                    embedding_id,
                    representation_id,
                    representation_type,
                    similarity,
                }
            },
        )
        .collect())
}

/// Count embedding coverage for a project.
///
/// All reads join through `memories` to enforce project-scope (defence-in-depth).
pub(crate) fn embedding_status(
    conn: &rusqlite::Connection,
    project_id: &ProjectId,
) -> Result<EmbeddingStatus> {
    // Total active memories.
    let total_active_memories: u64 = conn.query_row(
        "SELECT COUNT(*) FROM memories WHERE project_id = ?1 AND status = 'active'",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Embeddable representations: summary + compressed for active memories.
    // (`RepresentationDepth::Compressed` serialises as `"compressed"` — PRD §6.2.)
    let embeddable_representations: u64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_representations mr
         JOIN memories m ON m.id = mr.memory_id
         WHERE m.project_id = ?1
           AND m.status = 'active'
           AND mr.representation_type IN ('summary', 'compressed')",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Embedded: embeddable representations that have an active embedding.
    let embedded_representations: u64 = conn.query_row(
        "SELECT COUNT(DISTINCT e.representation_id)
         FROM memory_embeddings e
         JOIN memories m ON m.id = e.memory_id
         WHERE m.project_id = ?1
           AND m.status = 'active'
           AND e.status = 'active'",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Stale embeddings (belonging to this project's memories).
    let stale_embeddings: u64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_embeddings e
         JOIN memories m ON m.id = e.memory_id
         WHERE m.project_id = ?1 AND e.status = 'stale'",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Failed jobs (belonging to this project's memories).
    let failed_jobs: u64 = conn.query_row(
        "SELECT COUNT(*) FROM embedding_jobs ej
         JOIN memories m ON m.id = ej.memory_id
         WHERE m.project_id = ?1 AND ej.status = 'failed'",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Distinct representations with at least one active OR stale embedding —
    // used to compute `missing` without double-subtracting representations that
    // happen to have both (e.g. one row from an old provider + one new).
    let covered_representations: u64 = conn.query_row(
        "SELECT COUNT(DISTINCT e.representation_id)
         FROM memory_embeddings e
         JOIN memories m ON m.id = e.memory_id
         WHERE m.project_id = ?1
           AND m.status = 'active'
           AND e.status IN ('active', 'stale')",
        rusqlite::params![project_id.as_str()],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // Dominant provider + model by active embedding count.
    let (provider, model, dimensions) = query_dominant_provider(conn, project_id)?;

    let missing_embeddings = embeddable_representations.saturating_sub(covered_representations);

    Ok(EmbeddingStatus {
        project_id: project_id.clone(),
        provider,
        model,
        dimensions,
        total_active_memories,
        embeddable_representations,
        embedded_representations,
        stale_embeddings,
        failed_jobs,
        missing_embeddings,
    })
}

// === PRIVATE HELPERS ===

/// Encode a `&[f32]` as little-endian bytes for BLOB storage.
fn encode_vector(vector: &[f32]) -> Vec<u8> {
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
fn decode_vector(blob: &[u8]) -> Result<Vec<f32>> {
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
fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
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
fn compute_vector_hash(vector: &[f32]) -> String {
    let bytes = encode_vector(vector);
    let digest = Sha256::digest(&bytes);
    hex::encode(digest)
}

/// Generate an RFC-3339 timestamp string for `now`.
fn rfc3339_now() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(StoreError::Time)
}

/// Map an ID parse error into a `StoreError::Sqlite` (consistent with other helpers).
fn invalid_id_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}

/// Query the dominant (most-common) provider/model/dimensions among active embeddings.
fn query_dominant_provider(
    conn: &rusqlite::Connection,
    project_id: &ProjectId,
) -> Result<(Option<String>, Option<String>, Option<usize>)> {
    let mut stmt = conn.prepare(
        "SELECT e.provider, e.model, e.dimensions, COUNT(*) AS cnt
         FROM memory_embeddings e
         JOIN memories m ON m.id = e.memory_id
         WHERE m.project_id = ?1 AND e.status = 'active'
         GROUP BY e.provider, e.model, e.dimensions
         ORDER BY cnt DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![project_id.as_str()])?;
    if let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let model: String = row.get(1)?;
        let dims: i64 = row.get(2)?;
        Ok((Some(provider), Some(model), Some(dims as usize)))
    } else {
        Ok((None, None, None))
    }
}

/// Generate a ULID-based job ID for embedding_jobs.
#[allow(dead_code)] // used by CLI (PR4)
pub(crate) fn new_job_id() -> String {
    format!("job_{}", Ulid::new())
}

// === STORE METHODS: EMBEDDING HELPERS ===

impl Store {
    /// Fetch the `memory_representations.id` column for a given (memory, depth) pair.
    ///
    /// Returns `Some(id)` when a row exists for the requested depth, `None` otherwise.
    #[allow(dead_code)] // Wave 3 will wire CLI callers
    pub fn repr_id_for_depth(
        &self,
        memory_id: &MemoryId,
        depth: RepresentationDepth,
    ) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memory_representations
             WHERE memory_id = ?1 AND representation_type = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![memory_id.as_str(), depth.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Return `true` if an active embedding already exists for the given
    /// `(representation_id, provider, model)` triple.
    #[allow(dead_code)] // Wave 3 will wire CLI callers
    pub fn has_active_embedding(&self, repr_id: &str, provider: &str, model: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_embeddings
             WHERE representation_id = ?1
               AND provider = ?2
               AND model = ?3
               AND status = 'active'",
            rusqlite::params![repr_id, provider, model],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Insert a failed `embedding_jobs` row so `embeddings status` can surface it.
    #[allow(dead_code)] // Wave 3 will wire CLI callers
    pub fn record_failed_embedding_job(
        &mut self,
        memory_id: &MemoryId,
        repr_id: &str,
        depth: RepresentationDepth,
        provider: &str,
        model: &str,
        error: &str,
    ) -> Result<()> {
        let job_id = format!("job_{}", Ulid::new());
        let now_str = rfc3339_now()?;
        self.conn.execute(
            "INSERT INTO embedding_jobs
                (id, memory_id, representation_id, representation_type,
                 provider, model, status, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'failed', ?7, ?8, ?8)",
            rusqlite::params![
                job_id,
                memory_id.as_str(),
                repr_id,
                depth.as_str(),
                provider,
                model,
                error,
                now_str,
            ],
        )?;
        Ok(())
    }
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
