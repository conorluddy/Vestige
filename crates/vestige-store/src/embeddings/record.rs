//! Embedding write path — record / mark stale / delete.
//!
//! All functions take a `&rusqlite::Connection` and are crate-private; the
//! `Store` impl in `crate::lib` forwards to them.

use vestige_core::EmbeddingId;

use crate::Result;

use super::{compute_vector_hash, encode_vector, rfc3339_now, NewEmbedding};

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
