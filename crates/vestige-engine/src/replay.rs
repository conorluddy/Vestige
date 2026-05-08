//! Trace replay — re-run a stored query against the current store and provider.
//!
//! [`replay_trace`] is the single entry point. It:
//!
//! 1. Loads the original [`TraceDetail`] from the store (project-scoped).
//! 2. Re-runs the same query through the engine's existing search paths.
//! 3. Computes a structured diff (added / removed / score_changes) by comparing
//!    original and current `result_ids`.
//! 4. Writes **exactly one** new `query_events` row tagged with `replay_of` in
//!    `params_json`. The original trace is **never mutated**.
//! 5. Returns a [`ReplayResult`] matching PRD §10.3.
//!
//! # One trace per replay
//!
//! The existing `search_lexical` / `search_hybrid` / `search_semantic` helpers
//! each write their own trace row. To keep the replay as exactly **one** new row
//! (the one tagged with `replay_of`), this module calls the underlying store
//! operations directly and writes the single replay-tagged trace itself.
//!
//! # Provider mismatch
//!
//! - Original mode was `lexical` → no provider expected → `provider_match = true`.
//! - Original mode was `semantic` or `hybrid` and current provider is `None` →
//!   run lexical-only, set `provider_match = false`, `mode_fallback = true`.
//! - Original mode was `semantic` or `hybrid` and provider name/model differ →
//!   `provider_match = false`; the replay still runs (with current provider if
//!   available, or lexical fallback if `None`).
//! - Provider name and model both match → `provider_match = true`.
//!
//! # Original trace immutability
//!
//! The original row is read via [`get_trace`] and is never written. All writes
//! go through the normal [`write_trace`] path which produces a fresh
//! `trace_<ULID>`. The `replay_of` field in `params_json` links back to the
//! original without touching it.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use serde::Serialize;
use vestige_core::{
    merge_hits, normalise_cosine, normalise_fts, project_card, rank_hits, sanitize_fts_query,
    HybridOpts, HybridScore, MemoryId, MemoryType, ProjectId, ScoredCard, SearchFilter, SearchHit,
    SearchMode, SemanticHit, TraceId,
};
use vestige_embed::EmbeddingProvider;
use vestige_store::{Store, VectorFilter};

use crate::error::{EngineError, Result};
use crate::trace::{elapsed_since, start_timer, write_trace, Caller, TraceKind, TracePayload};
use crate::trace_read::{get_trace, TraceDetail};

// === PUBLIC TYPES ===

/// Result of replaying a stored query trace.
///
/// Matches the `replay` output shape in PRD §10.3.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    /// The original trace ID that was replayed.
    pub trace_id: String,
    /// Original results (IDs + scores in original order).
    pub original: ReplayResultSet,
    /// Current results (IDs + scores, ordered by score descending then ID for
    /// deterministic output in tests).
    pub current: ReplayResultSet,
    /// Diff between original and current result sets.
    pub diff: ReplayDiff,
    /// `true` when the current provider name and model match the original.
    /// Always `true` when the original mode was lexical (no provider expected).
    pub provider_match: bool,
    /// `true` when replay had to fall back to a different mode because the
    /// original required a provider that is now absent or mismatched.
    pub mode_fallback: bool,
    /// The new `trace_<ULID>` written by this replay.
    pub replay_trace_id: String,
    /// Number of active memories in the current store (corpus size now).
    pub corpus_size: u64,
}

/// A result set — ordered list of IDs with a parallel list of scores.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayResultSet {
    /// Ordered result memory IDs.
    pub result_ids: Vec<String>,
    /// Scores parallel to `result_ids`. Empty when scores were not recorded.
    pub scores: Vec<f64>,
}

/// Set-diff between original and current result sets.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayDiff {
    /// Memory IDs present in `current` but not in `original`.
    pub added: Vec<String>,
    /// Memory IDs present in `original` but not in `current`.
    pub removed: Vec<String>,
    /// Memories present in both sets whose score changed.
    pub score_changes: Vec<ScoreChange>,
}

/// A score change for a single memory ID across original and current.
#[derive(Debug, Clone, Serialize)]
pub struct ScoreChange {
    /// Memory ID whose score changed.
    pub id: String,
    /// `current_score - original_score` (positive = improved).
    pub delta: f64,
}

// === PUBLIC API ===

/// Re-run a stored query trace and diff its results against the current store.
///
/// # Parameters
///
/// - `store` — the project's open store.
/// - `provider` — the current embedding provider, or `None` if unavailable.
/// - `project_id` — used for project-scope enforcement; replaying a trace from
///   a different project returns [`EngineError::TraceNotFound`].
/// - `trace_id` — the `trace_<ULID>` to replay.
/// - `caller` — the surface that initiated the replay (CLI for now; MCP in M6).
///
/// # Errors
///
/// - [`EngineError::TraceNotFound`] — `trace_id` does not exist in this
///   project.
/// - [`EngineError::Validation`] — `trace_id` could not be parsed.
/// - [`EngineError::Store`] — SQLite failure.
/// - [`EngineError::Embed`] — provider failed to embed the query.
pub fn replay_trace(
    store: &Store,
    provider: Option<&dyn EmbeddingProvider>,
    project_id: &ProjectId,
    trace_id: &TraceId,
    caller: Caller,
) -> Result<ReplayResult> {
    // 1. Load original — project-scoped; returns TraceNotFound on miss.
    let original = get_trace(store, project_id, trace_id)?;

    let query_text = original.query.clone().unwrap_or_default();
    let mode_requested = parse_search_mode(original.mode_requested.as_deref());
    let limit = extract_limit(&original).unwrap_or(10);
    let type_filter = extract_type_filter(&original);

    // 2. Determine provider match.
    let requires_provider = matches!(
        mode_requested,
        Some(SearchMode::Semantic) | Some(SearchMode::Hybrid)
    );
    let provider_match = compute_provider_match(requires_provider, &original, provider);

    // 3. Re-run the search (no inner trace write — we write one replay-tagged row below).
    let t0 = start_timer();
    let (current_scored, effective_mode, mode_fallback) = execute_search(
        store,
        project_id,
        &query_text,
        mode_requested,
        limit,
        type_filter,
        provider,
        requires_provider,
    )?;
    let latency = elapsed_since(t0);

    // 4. Build current result set (stable order: sort by score desc, then id asc).
    let mut current_pairs: Vec<(String, f64)> = current_scored
        .iter()
        .map(|c| (c.card.id.as_str().to_string(), c.score))
        .collect();
    current_pairs.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let current_ids: Vec<String> = current_pairs.iter().map(|(id, _)| id.clone()).collect();
    let current_scores: Vec<f64> = current_pairs.iter().map(|(_, s)| *s).collect();

    // 5. Compute diff.
    let diff = compute_diff(
        &original.result_ids,
        &original.result_scores,
        &current_ids,
        &current_scores,
    );

    // 6. Corpus size (best-effort — zero on store failure).
    let corpus_size = store
        .memory_counts(project_id)
        .map(|c| c.active.max(0) as u64)
        .unwrap_or(0);

    // 7. Write a single replay-tagged trace row.
    let replay_params = build_replay_params(limit, type_filter, trace_id.as_str());
    let current_mem_ids: Vec<MemoryId> = current_ids
        .iter()
        .filter_map(|id| MemoryId::from_str(id).ok())
        .collect();

    write_trace(
        store,
        &TracePayload {
            project_id,
            kind: TraceKind::Search,
            mode_requested,
            mode_resolved: Some(effective_mode),
            query_text: Some(&query_text),
            params_json: Some(replay_params),
            caller,
            provider: provider.map(|p| p.provider_name()),
            provider_model: provider.map(|p| p.model_name()),
            result_ids: Some(&current_mem_ids),
            result_scores: Some(&current_scores),
            latency,
        },
    );

    // 8. Retrieve the ID of the trace we just wrote (last insert for this project).
    let replay_trace_id = fetch_last_trace_id(store, project_id)?;

    Ok(ReplayResult {
        trace_id: trace_id.as_str().to_string(),
        original: ReplayResultSet {
            result_ids: original.result_ids.clone(),
            scores: original.result_scores.clone(),
        },
        current: ReplayResultSet {
            result_ids: current_ids,
            scores: current_scores,
        },
        diff,
        provider_match,
        mode_fallback,
        replay_trace_id,
        corpus_size,
    })
}

// === PRIVATE HELPERS ===

/// Parse a mode string from the stored trace into a [`SearchMode`] variant.
fn parse_search_mode(mode: Option<&str>) -> Option<SearchMode> {
    match mode {
        Some("lexical") => Some(SearchMode::Lexical),
        Some("semantic") => Some(SearchMode::Semantic),
        Some("hybrid") => Some(SearchMode::Hybrid),
        _ => None,
    }
}

/// Extract the `limit` from a trace's `params_json`.
fn extract_limit(detail: &TraceDetail) -> Option<u32> {
    detail
        .params
        .as_ref()
        .and_then(|p| p.get("limit"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

/// Extract the `type_filter` from a trace's `params_json`.
fn extract_type_filter(detail: &TraceDetail) -> Option<MemoryType> {
    detail
        .params
        .as_ref()
        .and_then(|p| p.get("type_filter"))
        .and_then(|v| v.as_str())
        .and_then(|s| MemoryType::from_str(s).ok())
}

/// Determine whether the current provider matches the original trace's provider.
///
/// - If `requires_provider` is false (lexical original), always `true`.
/// - If `requires_provider` is true and current `provider` is `None`, `false`.
/// - If both names + models match, `true`; otherwise `false`.
fn compute_provider_match(
    requires_provider: bool,
    original: &TraceDetail,
    provider: Option<&dyn EmbeddingProvider>,
) -> bool {
    if !requires_provider {
        return true;
    }
    let Some(p) = provider else {
        return false;
    };
    let orig_provider = original.provider.as_deref().unwrap_or("");
    let orig_model = original.provider_model.as_deref().unwrap_or("");
    p.provider_name() == orig_provider && p.model_name() == orig_model
}

/// Execute the replay search WITHOUT writing a trace row.
///
/// Mirrors the logic in `search.rs` but skips `write_trace`. The caller
/// (`replay_trace`) writes the single replay-tagged trace row after this
/// function returns. Deduplicating to one row keeps the audit log clean.
///
/// Returns `(scored_cards, effective_mode, mode_fallback)`.
#[allow(clippy::too_many_arguments)]
fn execute_search(
    store: &Store,
    project_id: &ProjectId,
    query_text: &str,
    mode_requested: Option<SearchMode>,
    limit: u32,
    type_filter: Option<MemoryType>,
    provider: Option<&dyn EmbeddingProvider>,
    requires_provider: bool,
) -> Result<(Vec<ScoredCard>, SearchMode, bool)> {
    // Provider missing + originally required → lexical fallback, flag it.
    if requires_provider && provider.is_none() {
        let scored = run_lexical(store, project_id, query_text, type_filter, limit)?;
        return Ok((scored, SearchMode::Lexical, true));
    }

    match mode_requested {
        None | Some(SearchMode::Lexical) => {
            let scored = run_lexical(store, project_id, query_text, type_filter, limit)?;
            Ok((scored, SearchMode::Lexical, false))
        }
        Some(SearchMode::Semantic) => {
            let p = provider.expect("checked above: provider present when semantic requested");
            let scored = run_semantic(store, project_id, query_text, type_filter, limit, p)?;
            Ok((scored, SearchMode::Semantic, false))
        }
        Some(SearchMode::Hybrid) => {
            let p = provider.expect("checked above: provider present when hybrid requested");
            let (scored, fell_back) =
                run_hybrid(store, project_id, query_text, type_filter, limit, p)?;
            let eff_mode = if fell_back {
                SearchMode::Lexical
            } else {
                SearchMode::Hybrid
            };
            Ok((scored, eff_mode, fell_back))
        }
    }
}

// --- Inner search runners (no trace writes) ---

fn run_lexical(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
) -> Result<Vec<ScoredCard>> {
    let cleaned = sanitize_fts_query(query);
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let hits = store.search_memories(
        project_id,
        &cleaned,
        &SearchFilter {
            r#type: type_filter,
            limit: Some(limit),
            ..Default::default()
        },
    )?;
    Ok(rank_hits(hits))
}

fn run_semantic(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
) -> Result<Vec<ScoredCard>> {
    let status = store.embedding_status(project_id)?;
    if status.embedded_representations == 0 {
        return Ok(Vec::new());
    }

    let query_vec = provider.embed(query)?;
    let filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let raw_hits = store.nearest_neighbours(project_id, &query_vec, limit, &filter)?;

    let mut scored = Vec::with_capacity(raw_hits.len());
    for hit in &raw_hits {
        if let Some(fetched) = store.get_memory(&hit.memory_id)? {
            let similarity = hit.similarity.clamp(0.0, 1.0);
            scored.push(ScoredCard {
                card: project_card(&fetched),
                score: similarity,
                score_parts: Some(HybridScore {
                    fts: 0.0,
                    vector: similarity,
                    importance: 0.0,
                    type_boost: 0.0,
                    total: similarity,
                }),
            });
        }
    }
    Ok(scored)
}

fn run_hybrid(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    provider: &dyn EmbeddingProvider,
) -> Result<(Vec<ScoredCard>, bool)> {
    let status = store.embedding_status(project_id)?;

    // Check for mismatch or missing embeddings → lexical fallback.
    let has_mismatch = {
        let stored_provider = status.provider.as_deref();
        let stored_model = status.model.as_deref().unwrap_or("?");
        let stored_dims = status.dimensions.unwrap_or(0);
        stored_provider.is_some_and(|sp| {
            sp != provider.provider_name()
                || stored_model != provider.model_name()
                || stored_dims != provider.dimensions()
        })
    };

    if status.embedded_representations == 0 || has_mismatch {
        let scored = run_lexical(store, project_id, query, type_filter, limit)?;
        return Ok((scored, true));
    }

    // Overfetch factor mirrors `search.rs`.
    let per_leg = limit
        .saturating_mul(4) // HYBRID_OVERFETCH_MULTIPLIER
        .max(32); // HYBRID_OVERFETCH_FLOOR

    let cleaned = sanitize_fts_query(query);
    let lexical_hits: Vec<SearchHit> = if cleaned.is_empty() {
        Vec::new()
    } else {
        store.search_memories(
            project_id,
            &cleaned,
            &SearchFilter {
                r#type: type_filter,
                limit: Some(per_leg),
                ..Default::default()
            },
        )?
    };

    let query_vec = provider.embed(query)?;
    let vector_filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let vector_raw = store.nearest_neighbours(project_id, &query_vec, per_leg, &vector_filter)?;

    let semantic_hits: Vec<SemanticHit> = vector_raw
        .iter()
        .map(|h| SemanticHit {
            memory_id: h.memory_id.clone(),
            representation_type: h.representation_type.clone(),
            similarity: h.similarity,
        })
        .collect();

    let mut seen_ids: HashSet<MemoryId> = lexical_hits
        .iter()
        .map(|h| h.fetched.memory.id.clone())
        .collect();
    let mut candidates: Vec<SearchHit> = lexical_hits.clone();

    for sem_hit in &semantic_hits {
        if !seen_ids.insert(sem_hit.memory_id.clone()) {
            continue;
        }
        if let Some(fetched) = store.get_memory(&sem_hit.memory_id)? {
            candidates.push(SearchHit { fetched, bm25: 0.0 });
        }
    }

    let fts_scores = normalise_fts(&lexical_hits);
    let vector_scores = normalise_cosine(&semantic_hits);
    let opts = HybridOpts {
        limit,
        ..HybridOpts::default()
    };
    let scored = merge_hits(candidates, &fts_scores, &vector_scores, &opts);
    Ok((scored, false))
}

/// Compute the diff between original and current result ID lists.
///
/// Diff is set-based (not order-sensitive):
/// - `added` — IDs in `current` but not in `original`.
/// - `removed` — IDs in `original` but not in `current`.
/// - `score_changes` — IDs in both with differing scores (delta = current - original).
///
/// All output lists are sorted by ID for determinism.
fn compute_diff(
    original_ids: &[String],
    original_scores: &[f64],
    current_ids: &[String],
    current_scores: &[f64],
) -> ReplayDiff {
    let original_set: HashSet<&str> = original_ids.iter().map(String::as_str).collect();
    let current_set: HashSet<&str> = current_ids.iter().map(String::as_str).collect();

    let original_score_map: HashMap<&str, f64> = original_ids
        .iter()
        .zip(original_scores.iter())
        .map(|(id, &score)| (id.as_str(), score))
        .collect();

    let current_score_map: HashMap<&str, f64> = current_ids
        .iter()
        .zip(current_scores.iter())
        .map(|(id, &score)| (id.as_str(), score))
        .collect();

    let mut added: Vec<String> = current_set
        .difference(&original_set)
        .map(|s| s.to_string())
        .collect();
    added.sort();

    let mut removed: Vec<String> = original_set
        .difference(&current_set)
        .map(|s| s.to_string())
        .collect();
    removed.sort();

    let mut score_changes: Vec<ScoreChange> = original_set
        .intersection(&current_set)
        .filter_map(|id| {
            let orig_score = *original_score_map.get(id)?;
            let curr_score = *current_score_map.get(id)?;
            let delta = curr_score - orig_score;
            // Only surface changes above floating-point noise.
            if delta.abs() < f64::EPSILON {
                None
            } else {
                Some(ScoreChange {
                    id: id.to_string(),
                    delta,
                })
            }
        })
        .collect();
    // Sort by ID for determinism.
    score_changes.sort_by(|a, b| a.id.cmp(&b.id));

    ReplayDiff {
        added,
        removed,
        score_changes,
    }
}

/// Build the `params_json` string for a replay trace row.
///
/// Includes `replay_of` so the chain is inspectable.
fn build_replay_params(limit: u32, type_filter: Option<MemoryType>, replay_of: &str) -> String {
    #[derive(Serialize)]
    struct ReplayParams<'a> {
        limit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        type_filter: Option<&'a str>,
        replay_of: &'a str,
    }
    serde_json::to_string(&ReplayParams {
        limit,
        type_filter: type_filter.map(|t| t.as_str()),
        replay_of,
    })
    .unwrap_or_default()
}

/// Fetch the `id` of the most recently written `query_events` row for
/// `project_id`.
fn fetch_last_trace_id(store: &Store, project_id: &ProjectId) -> Result<String> {
    store
        .fetch_last_trace_id(project_id.as_str())
        .map_err(EngineError::Store)?
        .ok_or_else(|| EngineError::Validation {
            message: "replay trace write succeeded but row not retrievable".to_string(),
        })
}
