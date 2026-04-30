//! Embedding ingest orchestration. Iterates active memories or a single
//! memory's representations, asking an `EmbeddingProvider` to embed each
//! one and writing the result via `Store`. CLI's `vestige embed` is a thin
//! shell over this.

use serde::Serialize;

use vestige_core::{FetchedMemory, ListFilter, MemoryId, ProjectId, RepresentationDepth};
use vestige_embed::EmbeddingProvider;
use vestige_store::{NewEmbedding, Store};

use crate::error::Result;

// === TYPES ===

/// What happened to a single (memory, representation) during an embed run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbedOutcome {
    /// Vector was generated and persisted.
    Embedded,
    /// An active, current embedding already exists — skipped.
    Unchanged,
    /// Memory has no representation of this depth — skipped.
    NoRepr,
    /// Would embed (dry-run only).
    WouldEmbed,
    /// Embedding failed; a failed job row was recorded.
    Failed,
}

/// Result for a single (memory, representation) embed attempt.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedResult {
    pub memory_id: MemoryId,
    pub representation_type: String,
    pub outcome: EmbedOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// === PUBLIC API ===

/// Embed the requested representation depths for a single fetched memory.
///
/// Iterates `depths` in order, resolving the representation row's DB id via
/// `store.repr_id_for_depth`, checking for an existing active embedding via
/// `store.has_active_embedding`, and generating a new one if absent. Dry-run
/// mode reports `WouldEmbed` without writing.
///
/// Returns one `EmbedResult` per (memory, depth) pair.
pub fn embed_memory_representations(
    store: &mut Store,
    fetched: &FetchedMemory,
    provider: &dyn EmbeddingProvider,
    depths: &[RepresentationDepth],
    dry_run: bool,
) -> Result<Vec<EmbedResult>> {
    let mut results = Vec::new();
    let memory_id = &fetched.memory.id;

    for &depth in depths {
        let repr = fetched.representations.iter().find(|r| r.depth == depth);

        // No representation of this depth → nothing to embed.
        let Some(repr) = repr else {
            results.push(EmbedResult {
                memory_id: memory_id.clone(),
                representation_type: depth.as_str().to_owned(),
                outcome: EmbedOutcome::NoRepr,
                error: None,
            });
            continue;
        };

        // Resolve the DB row id for this (memory, depth) pair.
        let repr_id = match store.repr_id_for_depth(memory_id, depth)? {
            Some(id) => id,
            None => {
                results.push(EmbedResult {
                    memory_id: memory_id.clone(),
                    representation_type: depth.as_str().to_owned(),
                    outcome: EmbedOutcome::NoRepr,
                    error: None,
                });
                continue;
            }
        };

        // Check for an existing active embedding (skip the check in dry-run).
        if !dry_run
            && store.has_active_embedding(
                &repr_id,
                provider.provider_name(),
                provider.model_name(),
            )?
        {
            results.push(EmbedResult {
                memory_id: memory_id.clone(),
                representation_type: depth.as_str().to_owned(),
                outcome: EmbedOutcome::Unchanged,
                error: None,
            });
            continue;
        }

        if dry_run {
            results.push(EmbedResult {
                memory_id: memory_id.clone(),
                representation_type: depth.as_str().to_owned(),
                outcome: EmbedOutcome::WouldEmbed,
                error: None,
            });
            continue;
        }

        // Generate and persist the embedding.
        match provider.embed(&repr.content) {
            Ok(vector) => {
                store.record_embedding(&NewEmbedding {
                    memory_id,
                    representation_id: &repr_id,
                    representation_type: depth.as_str(),
                    provider: provider.provider_name(),
                    model: provider.model_name(),
                    vector: &vector,
                })?;
                results.push(EmbedResult {
                    memory_id: memory_id.clone(),
                    representation_type: depth.as_str().to_owned(),
                    outcome: EmbedOutcome::Embedded,
                    error: None,
                });
            }
            Err(e) => {
                let error_msg = e.to_string();
                // Best-effort: record a failed job row for `embeddings status`.
                let _ = store.record_failed_embedding_job(
                    memory_id,
                    &repr_id,
                    depth,
                    provider.provider_name(),
                    provider.model_name(),
                    &error_msg,
                );
                results.push(EmbedResult {
                    memory_id: memory_id.clone(),
                    representation_type: depth.as_str().to_owned(),
                    outcome: EmbedOutcome::Failed,
                    error: Some(error_msg),
                });
            }
        }
    }

    Ok(results)
}

/// Embed every active memory in the project for the given representation depths.
///
/// Iterates `store.list_memories` (active only) and calls
/// [`embed_memory_representations`] for each, concatenating all results.
pub fn embed_all(
    store: &mut Store,
    project_id: &ProjectId,
    provider: &dyn EmbeddingProvider,
    depths: &[RepresentationDepth],
    dry_run: bool,
) -> Result<Vec<EmbedResult>> {
    let memories = store.list_memories(project_id, &ListFilter::default())?;

    let mut results: Vec<EmbedResult> = Vec::new();
    for fetched in &memories {
        // list_memories with default filter already excludes deleted; guard
        // defensively in case the filter semantics change.
        if fetched.memory.status != vestige_core::MemoryStatus::Active {
            continue;
        }
        let partial = embed_memory_representations(store, fetched, provider, depths, dry_run)?;
        results.extend(partial);
    }

    Ok(results)
}
