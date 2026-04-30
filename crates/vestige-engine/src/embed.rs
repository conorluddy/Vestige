//! Embedding ingest orchestration.
//!
//! Iterates active memories (or a single memory's representations), asks an
//! [`EmbeddingProvider`] to embed each one, and persists the result via
//! [`Store`]. `vestige-cli`'s `embed` subcommand and `vestige-mcp`'s future
//! embed tool are both thin shells over the public functions here.
//!
//! # Idempotency
//!
//! Both [`embed_memory_representations`] and [`embed_all`] skip any
//! (memory, representation) pair that already has an active, current embedding
//! for the same provider/model, returning [`EmbedOutcome::Unchanged`]. Re-running
//! the ingest pipeline after adding new memories is safe and cheap.

use serde::Serialize;

use vestige_core::{FetchedMemory, ListFilter, MemoryId, ProjectId, RepresentationDepth};
use vestige_embed::EmbeddingProvider;
use vestige_store::{NewEmbedding, Store};

use crate::error::Result;

// === TYPES ===

/// What happened to a single (memory, representation) during an embed run.
///
/// Serialises as `snake_case` in the `--json` output of `vestige embed`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbedOutcome {
    /// A new vector was generated and persisted to the store.
    Embedded,
    /// An active embedding for this provider/model already exists — skipped.
    /// Re-running is safe; this variant means no work was needed.
    Unchanged,
    /// The memory has no representation at the requested depth — nothing to embed.
    NoRepr,
    /// Dry-run mode: would embed, but no writes were made.
    WouldEmbed,
    /// The provider failed; a failed-job row was recorded for `embeddings status`.
    Failed,
}

/// Result for a single (memory, representation) embed attempt.
///
/// One `EmbedResult` is produced for every (memory, depth) pair processed,
/// regardless of whether work was done. Aggregate these to build the summary
/// shown by `vestige embed --json`.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedResult {
    /// The memory that was processed.
    pub memory_id: MemoryId,
    /// The representation depth that was targeted (e.g. `"summary"`).
    pub representation_type: String,
    /// What happened to this (memory, representation) pair.
    pub outcome: EmbedOutcome,
    /// Set when `outcome` is [`EmbedOutcome::Failed`]; contains the provider error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// === PUBLIC API ===

/// Embed the requested representation depths for a single fetched memory.
///
/// Iterates `depths` in order, resolving each representation's DB row id via
/// `store.repr_id_for_depth`, checking for an existing active embedding via
/// `store.has_active_embedding`, and generating a new vector if absent.
///
/// **Idempotent**: if an active embedding for the same provider/model already
/// exists for a depth, that depth emits [`EmbedOutcome::Unchanged`] and no
/// provider call is made. Dry-run mode skips all writes and reports
/// [`EmbedOutcome::WouldEmbed`].
///
/// Returns one [`EmbedResult`] per `(memory, depth)` pair in `depths`.
///
/// # Errors
///
/// Returns [`EngineError::Store`] if any SQLite operation fails. Per-pair
/// provider failures are captured as [`EmbedOutcome::Failed`] entries (not
/// returned as `Err`) so the pipeline can continue with the remaining depths.
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
/// Iterates `store.list_memories` (active only, excluding soft-deleted) and
/// calls [`embed_memory_representations`] for each, concatenating all results
/// into a single flat list.
///
/// **Idempotent**: memories that already have a current embedding for each
/// requested depth are silently skipped with [`EmbedOutcome::Unchanged`].
/// Running this multiple times is safe; it only generates new work when new
/// memories or new representation depths are present.
///
/// # Errors
///
/// Returns [`EngineError::Store`] if any SQLite operation fails.
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
