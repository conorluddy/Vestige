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

use tracing::debug;
use vestige_core::{
    build_bundle, build_candidate_bundle, CandidateId, CandidateStatus, MemoryId, MemoryType,
    NewCandidate, NewMemory, ProjectId, RejectionReason,
};
use vestige_store::{CandidateFilter, Store};

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

/// A compact handle for a similar active memory returned from the dedup probe.
#[derive(Debug, Clone)]
pub struct SimilarMemory {
    /// Memory identifier.
    pub id: MemoryId,
    /// Derived short title (one_liner from the fetched memory).
    pub title: String,
    /// BM25 score (lower = closer match; FTS5 convention).
    pub score: f32,
}

/// A compact handle for a similar pending candidate returned from the dedup probe.
#[derive(Debug, Clone)]
pub struct SimilarCandidate {
    /// Candidate identifier.
    pub id: CandidateId,
    /// Short display title from the candidate row.
    pub title: String,
    /// BM25 score (lower = closer match; FTS5 convention).
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
/// the candidate body (sanitised to strip FTS5 special characters). If either
/// query fails (syntax error on unusual input, empty query), the error is
/// swallowed and an empty similar list is returned — dedup failure must never
/// block proposal (PRD §13).
///
/// # Errors
///
/// Returns [`EngineError::Core`] if `build_candidate_bundle` fails (e.g. empty
/// body). Returns [`EngineError::Store`] on SQLite write failure.
pub fn propose_candidate(
    store: &mut Store,
    project_id: &ProjectId,
    new_candidate: NewCandidate,
) -> Result<ProposeOutcome> {
    // Build the bundle — validates body, derives representations, generates ID.
    let bundle = build_candidate_bundle(new_candidate)?;

    let proposed_type = bundle.proposed_type;

    // Dedup probe: only worth running when body is non-trivial.
    let probe_body = &bundle.full_body;
    let (similar_memories, similar_candidates) =
        if probe_body.trim().len() < 8 || bundle.title.trim().len() < 8 {
            debug!(
                candidate_id = bundle.id.as_str(),
                "dedup probe skipped: body/title too short"
            );
            (vec![], vec![])
        } else {
            run_dedup_probe(store, project_id, probe_body, proposed_type)
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
/// 4. Write additional `memory_sources` rows: one per `CandidateSource` and one
///    reverse-provenance row with `source_type = "candidate"` (PRD §14).
/// 5. Call `store.mark_candidate_approved` (flips status, emits audit event).
///
/// # Transactionality
///
/// Steps 3 and 5 are two separate internally-transactional store calls. A
/// failure between them leaves the memory written but the candidate still
/// pending — re-running `approve` will attempt to write a duplicate memory row.
///
/// TODO(v0.3): wrap steps 3+5 in a single store-level transaction to eliminate
/// the window. For V0.2 the two-step approach is acceptable.
///
/// # Errors
///
/// - [`EngineError::CandidateNotFound`] — no row for `candidate_id`.
/// - [`EngineError::OutOfScope`] — candidate belongs to a different project.
/// - [`EngineError::CandidateNotPending`] — candidate is not `Pending`.
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

    // --- Step 4: write source rows ---

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

    // --- Step 5: flip candidate status ---
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

/// Run lexical dedup probes against active memories and pending candidates.
///
/// Any query or store error is swallowed and an empty list is returned — dedup
/// failure must never block proposal (PRD §13).
fn run_dedup_probe(
    store: &Store,
    project_id: &ProjectId,
    body: &str,
    proposed_type: MemoryType,
) -> (Vec<SimilarMemory>, Vec<SimilarCandidate>) {
    let Some(fts_query) = dedup_fts_query(body) else {
        debug!("dedup probe: empty FTS query after sanitise");
        return (vec![], vec![]);
    };

    // --- Probe 1: active memories of the same type ---
    use vestige_core::{SearchFilter, SearchHit};
    let similar_memories: Vec<SimilarMemory> = match store.search_memories(
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
                score: h.bm25 as f32,
            })
            .collect(),
        Err(e) => {
            debug!(error = %e, "dedup probe: memory search failed; continuing without similars");
            vec![]
        }
    };

    // --- Probe 2: pending candidates of the same type ---
    let filter = CandidateFilter {
        status: Some(CandidateStatus::Pending),
        proposed_type: Some(proposed_type),
        limit: Some(3),
        include_rejected: false,
    };
    let similar_candidates: Vec<SimilarCandidate> = match store
        .search_candidates_lexical(project_id, &fts_query, &filter)
    {
        Ok(hits) => hits
            .into_iter()
            .map(|h| SimilarCandidate {
                id: h.id,
                title: h.snippet,
                score: h.score,
            })
            .collect(),
        Err(e) => {
            debug!(error = %e, "dedup probe: candidate search failed; continuing without similars");
            vec![]
        }
    };

    (similar_memories, similar_candidates)
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
