//! Brute-force cosine nearest-neighbour scan over `memory_vectors`.

use std::str::FromStr;

use tracing::warn;

use vestige_core::{EmbeddingId, MemoryId, MemoryType, ProjectId};

use crate::Result;

use super::{cosine_similarity, decode_vector, invalid_id_err, VectorFilter, VectorHit};

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
