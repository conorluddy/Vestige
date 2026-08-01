//! Candidate inbox orchestration — propose, approve, reject.
//!
//! Three public functions bridge `vestige-core` domain types and
//! `vestige-store` persistence for the V0.2 assimilation inbox:
//!
//! - [`propose_candidate`] — dedup probe + insert.
//! - [`approve_candidate`] — promote a pending candidate to a full memory,
//!   writing reverse-provenance source rows (PRD §14).
//! - [`reject_candidate`] — flip status with a [`RejectionReason`].
//!
//! All three take `&ProjectId` and verify scope before any mutation.

use std::collections::HashSet;

use serde::Serialize;
use tracing::debug;
use vestige_core::{
    build_bundle, build_candidate_bundle, CandidateId, CandidateStatus, MemoryId, MemoryType,
    NewCandidate, NewMemory, ProjectId, RejectionReason,
};
use vestige_embed::EmbeddingProvider;
use vestige_store::{CandidateFilter, Store, VectorFilter};

use crate::error::{EngineError, Result};

// === PUBLIC TYPES ===

/// Return value from [`propose_candidate`].
///
/// `similar_memories` and `similar_candidates` are dedup hints — the caller
/// may surface them as warnings or structured JSON so the agent or user can
/// decide whether to suppress or proceed.
#[derive(Debug, Clone)]
pub struct ProposeOutcome {
    /// The newly inserted candidate.
    pub candidate_id: CandidateId,
    /// Always `Pending` immediately after proposal.
    pub status: CandidateStatus,
    /// Active memories with lexically similar content and the same type (up to 3).
    pub similar_memories: Vec<SimilarMemory>,
    /// Pending candidates with lexically similar content and the same type (up to 3).
    pub similar_candidates: Vec<SimilarCandidate>,
}

/// Which dedup leg(s) matched a given [`SimilarMemory`].
///
/// Candidates never carry this — the candidate leg is permanently
/// lexical-only (see [`run_dedup_probe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchedVia {
    /// Found only by the lexical (FTS5 BM25) leg.
    Lexical,
    /// Found only by the semantic (cosine similarity) leg.
    Semantic,
    /// Found by both legs — see [`merge_similar_memories`] for the merge rule.
    Both,
}

/// A compact handle for a similar active memory returned from the dedup probe.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarMemory {
    /// Memory identifier.
    pub id: MemoryId,
    /// Derived short title (one_liner from the fetched memory).
    pub title: String,
    /// Normalized score in [0, 1]; higher = closer match. Combines BM25 and
    /// cosine similarity onto a common scale — see [`run_dedup_probe`].
    pub score: f32,
    /// Which leg(s) of the dedup probe surfaced this memory.
    pub matched_via: MatchedVia,
}

/// A compact handle for a similar pending candidate returned from the dedup probe.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarCandidate {
    /// Candidate identifier.
    pub id: CandidateId,
    /// Short display title from the candidate row.
    pub title: String,
    /// Normalized score in [0, 1]; higher = closer match. Combines BM25 and
    /// cosine similarity onto a common scale — see [`run_dedup_probe`].
    pub score: f32,
}

/// Optional field overrides applied at approval time.
///
/// `None` fields fall back to the original candidate values.
#[derive(Debug, Clone, Default)]
pub struct ApprovalOverrides {
    /// Override the proposed memory type.
    pub proposed_type: Option<MemoryType>,
    /// Override the candidate body (becomes the memory body).
    pub body: Option<String>,
    /// Override importance. Falls back to the candidate's importance if `None`.
    pub importance: Option<f32>,
    /// Soft-delete and link this memory as superseded by the one being
    /// approved (issue #131). `None` leaves every existing memory untouched.
    pub supersedes: Option<MemoryId>,
}

/// Return value from [`approve_candidate`].
#[derive(Debug, Clone)]
pub struct ApprovalOutcome {
    /// The candidate that was approved.
    pub candidate_id: CandidateId,
    /// The memory row that was created from the candidate.
    pub memory_id: MemoryId,
}

// === PUBLIC API ===

/// Propose a new candidate with a dedup probe against active memories and
/// pending candidates of the same type.
///
/// # Dedup probe
///
/// Before inserting, runs two lexical FTS queries using the first ~80 chars of
/// the candidate body (sanitised to strip FTS5 special characters), plus an
/// optional semantic (cosine similarity) leg against active memories of the
/// same type when `embedding_provider` is `Some`. If any leg fails (syntax
/// error on unusual input, empty query, cold-start with no embeddings, embed
/// failure), the error is swallowed for that leg and it contributes no
/// results — dedup failure must never block proposal (PRD §13).
/// `embedding_provider = None` skips the semantic leg entirely (lexical-only,
/// matching pre-#133 behaviour byte-for-byte); this is the expected state
/// until provider construction is wired at the CLI/MCP call sites.
///
/// # Errors
///
/// Returns [`EngineError::Core`] if `build_candidate_bundle` fails (e.g. empty
/// body). Returns [`EngineError::Store`] on SQLite write failure.
pub fn propose_candidate(
    store: &mut Store,
    project_id: &ProjectId,
    new_candidate: NewCandidate,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> Result<ProposeOutcome> {
    // Build the bundle — validates body, derives representations, generates ID.
    let bundle = build_candidate_bundle(new_candidate)?;

    let proposed_type = bundle.proposed_type;

    // Dedup probe: only worth running when body is non-trivial. Short-circuits
    // both legs (lexical and semantic) — a body this short can't produce a
    // meaningful FTS query or a meaningful embedding either.
    let probe_body = &bundle.full_body;
    let (similar_memories, similar_candidates) =
        if probe_body.trim().len() < 8 || bundle.title.trim().len() < 8 {
            debug!(
                candidate_id = bundle.id.as_str(),
                "dedup probe skipped: body/title too short"
            );
            (vec![], vec![])
        } else {
            run_dedup_probe(
                store,
                project_id,
                probe_body,
                proposed_type,
                embedding_provider,
            )
        };

    // Insert candidate — after probe so it can't match itself.
    store.record_candidate(&bundle)?;

    Ok(ProposeOutcome {
        candidate_id: bundle.id,
        status: CandidateStatus::Pending,
        similar_memories,
        similar_candidates,
    })
}

/// Approve a pending candidate, creating a full memory with provenance.
///
/// Steps (conceptually atomic — see transactionality note below):
/// 1. Load candidate; verify project scope and pending status.
/// 2. Apply `overrides` (type, body, importance).
/// 3. Build a `MemoryBundle` and call `store.record_memory` (fires FTS triggers).
/// 4. If `overrides.supersedes` is set, call `store.supersede_memory` to
///    soft-delete and link the old memory (issue #131).
/// 5. Write additional `memory_sources` rows: one per `CandidateSource` and one
///    reverse-provenance row with `source_type = "candidate"` (PRD §14).
/// 6. Call `store.mark_candidate_approved` (flips status, emits audit event).
///
/// # Transactionality
///
/// Steps 3, 4, and 6 are separate internally-transactional store calls. A
/// failure between them leaves earlier steps applied but later ones not —
/// e.g. re-running `approve` after a failure between 3 and 6 will attempt to
/// write a duplicate memory row. The same accepted window covers step 4: a
/// failure between the memory being recorded (3) and the old memory being
/// superseded (4) leaves the new memory active but the old one un-superseded.
///
/// TODO(v0.3): wrap steps 3+6 (and now 4) in a single store-level transaction
/// to eliminate the window. For V0.2 the two-step approach is acceptable.
///
/// # Errors
///
/// - [`EngineError::CandidateNotFound`] — no row for `candidate_id`.
/// - [`EngineError::OutOfScope`] — candidate belongs to a different project.
/// - [`EngineError::CandidateNotPending`] — candidate is not `Pending`.
/// - [`EngineError::Validation`] — `overrides.supersedes` names a memory that
///   is not found, already deleted, or already superseded.
/// - [`EngineError::Core`] — `build_bundle` validation failure.
/// - [`EngineError::Store`] — any SQLite failure.
pub fn approve_candidate(
    store: &mut Store,
    project_id: &ProjectId,
    candidate_id: &CandidateId,
    overrides: ApprovalOverrides,
) -> Result<ApprovalOutcome> {
    // --- Step 1: load + validate ---
    let candidate =
        store
            .get_candidate(candidate_id)?
            .ok_or_else(|| EngineError::CandidateNotFound {
                id: candidate_id.as_str().to_string(),
            })?;

    if &candidate.project_id != project_id {
        return Err(EngineError::OutOfScope);
    }

    if candidate.status != CandidateStatus::Pending {
        return Err(EngineError::CandidateNotPending {
            status: candidate.status,
        });
    }

    // Belt-and-braces: if approved_memory_id is already set, we've somehow
    // already done step 5 but status was not flipped. Bail to avoid a second
    // memory row being written.
    if candidate.approved_memory_id.is_some() {
        return Err(EngineError::CandidateNotPending {
            status: candidate.status,
        });
    }

    // --- Step 2: resolve final field values from overrides ---
    let memory_type = overrides.proposed_type.unwrap_or(candidate.proposed_type);
    let body = overrides
        .body
        .as_deref()
        .unwrap_or(&candidate.full_body)
        .to_string();
    // importance stored as f32 in candidate; NewMemory takes f64
    let importance = overrides
        .importance
        .map(|v| v as f64)
        .unwrap_or_else(|| candidate.importance as f64)
        .clamp(0.0, 1.0);

    // --- Step 3: build memory bundle + persist ---
    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: memory_type,
            body: &body,
            importance,
            source: None, // extra sources written individually below
        },
    )?;

    let memory_id = bundle.memory.id.clone();
    store.record_memory(&bundle)?;

    // --- Step 4: optional supersede (issue #131) ---
    if let Some(old_id) = &overrides.supersedes {
        if !store.supersede_memory(old_id, &memory_id)? {
            return Err(EngineError::Validation {
                message: format!(
                    "approved {memory_id} but could not supersede `{old_id}` — it may not \
                     exist or is already deleted/superseded"
                ),
            });
        }
    }

    // --- Step 5: write source rows ---

    // Copy each CandidateSource from the candidate to memory_sources.
    for src in &candidate.sources {
        if let Err(e) = store.add_memory_source(
            &memory_id,
            &src.source_type,
            src.source_ref.as_deref(),
            src.source_content.as_deref(),
        ) {
            // Non-fatal: source rows are provenance metadata. Log and continue.
            debug!(
                memory_id = memory_id.as_str(),
                source_type = src.source_type.as_str(),
                error = %e,
                "failed to copy candidate source to memory; continuing"
            );
        }
    }

    // Mandatory reverse-provenance row (PRD §14).
    store.add_memory_source(&memory_id, "candidate", Some(candidate_id.as_str()), None)?;

    // --- Step 6: flip candidate status ---
    store.mark_candidate_approved(candidate_id, &memory_id)?;

    Ok(ApprovalOutcome {
        candidate_id: candidate_id.clone(),
        memory_id,
    })
}

/// Reject a pending candidate with an explicit reason.
///
/// Thin wrapper over `Store::mark_candidate_rejected` that enforces project
/// scope, pending-status guard, and the rule that `duplicate_of` may only be
/// set when `reason == Duplicate`.
///
/// # Errors
///
/// - [`EngineError::CandidateNotFound`] — no row for `candidate_id`.
/// - [`EngineError::OutOfScope`] — candidate belongs to a different project.
/// - [`EngineError::CandidateNotPending`] — candidate is not `Pending`.
/// - [`EngineError::Validation`] — `duplicate_of` provided with non-Duplicate reason.
/// - [`EngineError::Store`] — any SQLite failure.
pub fn reject_candidate(
    store: &mut Store,
    project_id: &ProjectId,
    candidate_id: &CandidateId,
    reason: RejectionReason,
    duplicate_of: Option<MemoryId>,
    review_note: Option<String>,
) -> Result<()> {
    // --- Load + validate ---
    let candidate =
        store
            .get_candidate(candidate_id)?
            .ok_or_else(|| EngineError::CandidateNotFound {
                id: candidate_id.as_str().to_string(),
            })?;

    if &candidate.project_id != project_id {
        return Err(EngineError::OutOfScope);
    }

    if candidate.status != CandidateStatus::Pending {
        return Err(EngineError::CandidateNotPending {
            status: candidate.status,
        });
    }

    // duplicate_of is only meaningful when the reason is Duplicate.
    if duplicate_of.is_some() && reason != RejectionReason::Duplicate {
        return Err(EngineError::Validation {
            message: format!("`duplicate_of` requires reason = `duplicate`, got `{reason}`"),
        });
    }

    store.mark_candidate_rejected(
        candidate_id,
        &reason,
        duplicate_of.as_ref(),
        review_note.as_deref(),
    )?;

    Ok(())
}

// === PRIVATE HELPERS ===

/// Build the FTS query string for the dedup probe.
///
/// Takes the first 80 bytes of `body` (byte-safe), strips FTS5 special
/// characters per-token, and joins up to 6 non-trivial tokens with ` OR `
/// so that FTS5 returns any document sharing at least one keyword — not an
/// AND intersection that would require every query term to appear in the
/// candidate document.
///
/// Stop-ish words (≤ 3 chars) are skipped to reduce noise. Returns `None`
/// if no usable tokens remain.
fn dedup_fts_query(body: &str) -> Option<String> {
    // Slice to 80 bytes at a valid UTF-8 boundary.
    let snippet = if body.len() > 80 {
        let mut end = 80;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        &body[..end]
    } else {
        body
    };

    // Sanitise each token (strip FTS5 special chars), keep tokens > 3 chars,
    // and take at most 6 to avoid an overly permissive OR query.
    let tokens: Vec<String> = snippet
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|t| t.len() > 3)
        .take(6)
        .map(|t| format!("\"{t}\""))
        .collect();

    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

/// Normalize a raw FTS5 BM25 score to `[0, 1]`, higher = closer match.
///
/// SQLite FTS5's `bm25()` returns negative values where lower (more negative)
/// means a better match. `exp(bm25/10)` lands in `(0, 1)` and increases as
/// the match improves, so `1 - that` is monotonic in the right direction. The
/// clamp guards the theoretically-positive-bm25 edge case.
fn normalize_bm25(bm25: f64) -> f32 {
    (1.0 - (bm25 / 10.0).exp()).clamp(0.0, 1.0) as f32
}

/// Normalize a raw cosine similarity (`[-1, 1]`, not clamped by the store) to
/// `[0, 1]`, higher = closer match. Mirrors `search_semantic`'s clamp
/// (`crates/vestige-engine/src/search.rs`).
fn normalize_cosine(similarity: f64) -> f32 {
    similarity.clamp(0.0, 1.0) as f32
}

/// Merge the lexical-leg and semantic-leg memory hits into a single ranked,
/// deduplicated list capped at 3.
///
/// Dedupes by [`MemoryId`]: a memory found by both legs is emitted once with
/// `matched_via = MatchedVia::Both`, keeping the lexical leg's score and
/// title (an arbitrary but consistent choice — the spec does not mandate
/// max/avg). Remaining lexical-only and semantic-only hits are interleaved
/// (lexical[0], semantic[0], lexical[1], semantic[1], ...) rather than
/// concatenated, so neither leg systematically crowds out the other in a
/// capped list. `both` hits sort first.
fn merge_similar_memories(
    lexical: Vec<SimilarMemory>,
    semantic: Vec<SimilarMemory>,
) -> Vec<SimilarMemory> {
    let semantic_ids: HashSet<MemoryId> = semantic.iter().map(|m| m.id.clone()).collect();

    let mut both = Vec::new();
    let mut lexical_only = Vec::new();
    for mut hit in lexical {
        if semantic_ids.contains(&hit.id) {
            hit.matched_via = MatchedVia::Both;
            both.push(hit);
        } else {
            lexical_only.push(hit);
        }
    }

    let both_ids: HashSet<MemoryId> = both.iter().map(|m| m.id.clone()).collect();
    let semantic_only: Vec<SimilarMemory> = semantic
        .into_iter()
        .filter(|m| !both_ids.contains(&m.id))
        .collect();

    let mut merged = both;
    let mut lexical_iter = lexical_only.into_iter();
    let mut semantic_iter = semantic_only.into_iter();
    loop {
        let took_lexical = lexical_iter.next().map(|h| merged.push(h)).is_some();
        let took_semantic = semantic_iter.next().map(|h| merged.push(h)).is_some();
        if !took_lexical && !took_semantic {
            break;
        }
    }

    merged.truncate(3);
    merged
}

/// Run lexical (and, when a provider is available, semantic) dedup probes
/// against active memories and pending candidates.
///
/// Any query, embed, or store error is swallowed and the affected leg
/// contributes no results — dedup failure must never block proposal
/// (PRD §13).
fn run_dedup_probe(
    store: &Store,
    project_id: &ProjectId,
    body: &str,
    proposed_type: MemoryType,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> (Vec<SimilarMemory>, Vec<SimilarCandidate>) {
    use vestige_core::{SearchFilter, SearchHit};

    // --- Lexical leg: active memories of the same type ---
    let lexical_memories: Vec<SimilarMemory> = match dedup_fts_query(body) {
        None => {
            debug!("dedup probe: empty FTS query after sanitise");
            vec![]
        }
        Some(fts_query) => match store.search_memories(
            project_id,
            &fts_query,
            &SearchFilter {
                r#type: Some(proposed_type),
                limit: Some(3),
                ..Default::default()
            },
        ) {
            Ok(hits) => hits
                .into_iter()
                .map(|h: SearchHit| SimilarMemory {
                    id: h.fetched.memory.id,
                    title: h
                        .fetched
                        .representations
                        .iter()
                        .find(|r| r.depth == vestige_core::RepresentationDepth::OneLiner)
                        .map(|r| r.content.clone())
                        .unwrap_or_default(),
                    score: normalize_bm25(h.bm25),
                    matched_via: MatchedVia::Lexical,
                })
                .collect(),
            Err(e) => {
                debug!(
                    error = %e,
                    "dedup probe: memory search failed; continuing without similars"
                );
                vec![]
            }
        },
    };

    // --- Semantic leg: active memories of the same type, by cosine similarity ---
    // `None` means commit 4's provider wiring hasn't landed at this call site
    // yet — that's a normal, expected state, not an error.
    let semantic_memories: Vec<SimilarMemory> = match embedding_provider {
        None => vec![],
        Some(provider) => run_semantic_dedup_leg(store, project_id, body, proposed_type, provider),
    };

    let similar_memories = merge_similar_memories(lexical_memories, semantic_memories);

    // --- Candidate leg: pending candidates of the same type ---
    // Candidates are never embedded — no vector index exists for pending
    // candidates, so this leg is permanently lexical-only.
    let similar_candidates: Vec<SimilarCandidate> = match dedup_fts_query(body) {
        None => vec![],
        Some(fts_query) => {
            let filter = CandidateFilter {
                status: Some(CandidateStatus::Pending),
                proposed_type: Some(proposed_type),
                limit: Some(3),
                include_rejected: false,
            };
            match store.search_candidates_lexical(project_id, &fts_query, &filter) {
                Ok(hits) => hits
                    .into_iter()
                    .map(|h| SimilarCandidate {
                        id: h.id,
                        title: h.snippet,
                        score: normalize_bm25(h.score as f64),
                    })
                    .collect(),
                Err(e) => {
                    debug!(
                        error = %e,
                        "dedup probe: candidate search failed; continuing without similars"
                    );
                    vec![]
                }
            }
        }
    };

    (similar_memories, similar_candidates)
}

/// Run the semantic-leg dedup probe: embed `body` and look up nearest
/// neighbours among active memories of `proposed_type`.
///
/// Mirrors `search_semantic`'s cold-start and error handling
/// (`crates/vestige-engine/src/search.rs`), except every failure is swallowed
/// rather than propagated with `?` — this is a best-effort dedup hint, not a
/// user-facing search (PRD §13).
fn run_semantic_dedup_leg(
    store: &Store,
    project_id: &ProjectId,
    body: &str,
    proposed_type: MemoryType,
    provider: &dyn EmbeddingProvider,
) -> Vec<SimilarMemory> {
    let status = match store.embedding_status(project_id) {
        Ok(status) => status,
        Err(e) => {
            debug!(error = %e, "dedup probe: embedding status lookup failed; continuing without semantic similars");
            return vec![];
        }
    };
    if status.embedded_representations == 0 {
        debug!("dedup probe: no embeddings for project; skipping semantic leg");
        return vec![];
    }

    let query_vec = match provider.embed(body) {
        Ok(v) => v,
        Err(e) => {
            debug!(error = %e, "dedup probe: embed failed; continuing without semantic similars");
            return vec![];
        }
    };

    let filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: Some(proposed_type),
    };
    let raw_hits = match store.nearest_neighbours(project_id, &query_vec, 3, &filter) {
        Ok(hits) => hits,
        Err(e) => {
            debug!(error = %e, "dedup probe: nearest-neighbour lookup failed; continuing without semantic similars");
            return vec![];
        }
    };

    let mut similar = Vec::with_capacity(raw_hits.len());
    for hit in &raw_hits {
        let fetched = match store.get_memory(&hit.memory_id) {
            Ok(Some(fetched)) => fetched,
            Ok(None) => continue,
            Err(e) => {
                debug!(error = %e, "dedup probe: memory fetch failed; skipping hit");
                continue;
            }
        };
        similar.push(SimilarMemory {
            id: fetched.memory.id.clone(),
            title: fetched
                .representations
                .iter()
                .find(|r| r.depth == vestige_core::RepresentationDepth::OneLiner)
                .map(|r| r.content.clone())
                .unwrap_or_default(),
            score: normalize_cosine(hit.similarity),
            matched_via: MatchedVia::Semantic,
        });
    }
    similar
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::{build_bundle, MemoryType, NewCandidate, NewMemory, ProjectId};
    use vestige_store::Store;

    fn open_store(tmp: &TempDir) -> Store {
        Store::open(tmp.path().join("memory.sqlite")).unwrap()
    }

    fn seed_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test Project", None, None)
            .unwrap();
    }

    fn new_candidate(project_id: ProjectId, body: &str, memory_type: MemoryType) -> NewCandidate {
        NewCandidate {
            project_id,
            proposed_type: memory_type,
            body: body.to_string(),
            rationale: Some("test rationale".to_string()),
            title_override: None,
            importance: 0.6,
            confidence: 0.8,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        }
    }

    fn seed_memory(
        store: &mut Store,
        project_id: &ProjectId,
        body: &str,
        memory_type: MemoryType,
    ) -> MemoryId {
        let bundle = build_bundle(
            project_id,
            NewMemory {
                r#type: memory_type,
                body,
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        let id = bundle.memory.id.clone();
        store.record_memory(&bundle).unwrap();
        id
    }

    // --- propose_candidate ---

    #[test]
    fn propose_returns_empty_similars_on_fresh_project() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("fresh-proj");
        seed_project(&mut store, &proj);

        let outcome = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Use Rust for all systems work.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        assert!(outcome.candidate_id.as_str().starts_with("cand_"));
        assert_eq!(outcome.status, CandidateStatus::Pending);
        assert!(outcome.similar_memories.is_empty());
        assert!(outcome.similar_candidates.is_empty());
    }

    #[test]
    fn propose_finds_similar_active_memory_by_keyword() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("dedup-mem");
        seed_project(&mut store, &proj);

        // Seed an existing memory with overlapping terms.
        seed_memory(
            &mut store,
            &proj,
            "SQLite is chosen as the canonical storage engine for Vestige.",
            MemoryType::Decision,
        );

        let outcome = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "SQLite canonical storage engine selected for reliability.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        assert!(
            !outcome.similar_memories.is_empty(),
            "should surface the existing memory as similar"
        );
    }

    #[test]
    fn propose_finds_similar_pending_candidate() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("dedup-cand");
        seed_project(&mut store, &proj);

        // First proposal lands in pending.
        propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Use tokio for async runtime in all future services.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        // Second proposal — near-duplicate.
        let outcome = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "tokio async runtime is the preferred choice.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        assert!(
            !outcome.similar_candidates.is_empty(),
            "should surface the first pending candidate as similar"
        );
    }

    // --- approve_candidate ---

    #[test]
    fn approve_creates_memory_visible_in_store() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-basic");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Always run cargo fmt before committing.",
                MemoryType::Preference,
            ),
            None,
        )
        .unwrap();

        let outcome = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap();

        let memory = store.get_memory(&outcome.memory_id).unwrap();
        assert!(memory.is_some(), "approved memory must be retrievable");
    }

    #[test]
    fn approve_writes_reverse_provenance_source_row() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-prov");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Prefer newtypes over bare strings for IDs.",
                MemoryType::Preference,
            ),
            None,
        )
        .unwrap();

        let outcome = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap();

        // Verify reverse-provenance row in memory_sources.
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM memory_sources
                 WHERE memory_id = ?1 AND source_type = 'candidate' AND source_ref = ?2",
                rusqlite::params![outcome.memory_id.as_str(), proposed.candidate_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "reverse-provenance source row must be present");
    }

    #[test]
    fn approve_flips_candidate_to_approved_with_memory_link() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-flip");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Use semantic versioning for all crate releases.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        let outcome = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap();

        let cand = store
            .get_candidate(&proposed.candidate_id)
            .unwrap()
            .unwrap();
        assert_eq!(cand.status, CandidateStatus::Approved);
        assert_eq!(cand.approved_memory_id.as_ref(), Some(&outcome.memory_id));
        assert!(cand.reviewed_at.is_some());
    }

    #[test]
    fn approve_already_approved_returns_not_pending_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-twice");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Feature flags must be cleaned up within one sprint.",
                MemoryType::Preference,
            ),
            None,
        )
        .unwrap();

        approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap();

        let err = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, EngineError::CandidateNotPending { .. }),
            "expected CandidateNotPending, got: {err}"
        );
    }

    #[test]
    fn approve_candidate_from_different_project_returns_out_of_scope() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);

        let proj_a = ProjectId::from_slug("scope-a");
        let proj_b = ProjectId::from_slug("scope-b");
        seed_project(&mut store, &proj_a);
        seed_project(&mut store, &proj_b);

        // Propose under project A.
        let proposed = propose_candidate(
            &mut store,
            &proj_a,
            new_candidate(
                proj_a.clone(),
                "Decision scoped to project A only.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        // Attempt to approve from project B.
        let err = approve_candidate(
            &mut store,
            &proj_b,
            &proposed.candidate_id,
            ApprovalOverrides::default(),
        )
        .unwrap_err();

        assert!(
            matches!(err, EngineError::OutOfScope),
            "expected OutOfScope, got: {err}"
        );
    }

    // --- approve_candidate with --supersedes (issue #131) ---

    #[test]
    fn approve_with_supersedes_supersedes_old_memory_in_one_call() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-supersedes");
        seed_project(&mut store, &proj);

        let old_id = seed_memory(
            &mut store,
            &proj,
            "Use polling for the daemon status check.",
            MemoryType::Decision,
        );

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Use a Unix-socket push for the daemon status check.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        let outcome = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides {
                supersedes: Some(old_id.clone()),
                ..Default::default()
            },
        )
        .unwrap();

        // Old memory is now deleted and linked to the newly approved one.
        let old_fetched = store.get_memory(&old_id).unwrap().unwrap();
        assert_eq!(
            old_fetched.memory.status,
            vestige_core::MemoryStatus::Deleted
        );
        assert_eq!(
            old_fetched.memory.superseded_by,
            Some(outcome.memory_id.clone())
        );

        // New memory is active and unaffected otherwise.
        let new_fetched = store.get_memory(&outcome.memory_id).unwrap().unwrap();
        assert_eq!(
            new_fetched.memory.status,
            vestige_core::MemoryStatus::Active
        );
    }

    #[test]
    fn approve_with_supersedes_unknown_memory_returns_validation_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("approve-supersedes-missing");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "A candidate that claims to supersede a bogus memory.",
                MemoryType::Note,
            ),
            None,
        )
        .unwrap();

        let bogus_old_id = MemoryId::new();
        let err = approve_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            ApprovalOverrides {
                supersedes: Some(bogus_old_id),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(
            matches!(err, EngineError::Validation { .. }),
            "expected Validation, got: {err}"
        );
    }

    // --- reject_candidate ---

    #[test]
    fn reject_flips_status_and_persists_reason() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("reject-basic");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Rewrite everything in Haskell for fun.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        reject_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            RejectionReason::NotDurable,
            None,
            Some("Joke proposal.".to_string()),
        )
        .unwrap();

        let cand = store
            .get_candidate(&proposed.candidate_id)
            .unwrap()
            .unwrap();
        assert_eq!(cand.status, CandidateStatus::Rejected);
        assert_eq!(cand.rejection_reason, Some(RejectionReason::NotDurable));
        assert_eq!(cand.review_note.as_deref(), Some("Joke proposal."));
    }

    #[test]
    fn reject_with_duplicate_of_persists_link() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("reject-dup");
        seed_project(&mut store, &proj);

        let existing_mem_id = seed_memory(
            &mut store,
            &proj,
            "Use SQLite for storage.",
            MemoryType::Decision,
        );

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "SQLite is the storage engine of choice.",
                MemoryType::Decision,
            ),
            None,
        )
        .unwrap();

        reject_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            RejectionReason::Duplicate,
            Some(existing_mem_id.clone()),
            None,
        )
        .unwrap();

        let cand = store
            .get_candidate(&proposed.candidate_id)
            .unwrap()
            .unwrap();
        assert_eq!(cand.status, CandidateStatus::Rejected);
        assert_eq!(cand.duplicate_of_memory_id.as_ref(), Some(&existing_mem_id));
    }

    #[test]
    fn reject_duplicate_of_with_non_duplicate_reason_is_validation_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("reject-val");
        seed_project(&mut store, &proj);

        let proposed = propose_candidate(
            &mut store,
            &proj,
            new_candidate(
                proj.clone(),
                "Something worth capturing here.",
                MemoryType::Note,
            ),
            None,
        )
        .unwrap();

        let bogus_mem_id = MemoryId::new();
        let err = reject_candidate(
            &mut store,
            &proj,
            &proposed.candidate_id,
            RejectionReason::Stale,
            Some(bogus_mem_id),
            None,
        )
        .unwrap_err();

        assert!(
            matches!(err, EngineError::Validation { .. }),
            "expected Validation, got: {err}"
        );
    }
}
