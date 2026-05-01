//! Project-scoped embedding coverage snapshot for `vestige embeddings status`.

use vestige_core::ProjectId;

use crate::Result;

use super::EmbeddingStatus;

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
