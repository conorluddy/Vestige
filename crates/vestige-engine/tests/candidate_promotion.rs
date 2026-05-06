//! Cross-crate integration tests for engine-level candidate promotion (V0.2).
//!
//! These tests exercise the full `propose → approve / reject` lifecycle at the
//! engine boundary, validating invariants that span `vestige-engine`,
//! `vestige-store`, and `vestige-core` together:
//!
//! - Approved candidates become recallable memories with correct provenance.
//! - Pending and rejected candidates stay invisible to memory search.
//! - Project-scope is enforced at the engine layer.
//! - Dedup hints fire on proposal for matching active memories and candidates.
//! - Cross-type similarity is suppressed (same-keyword, different type → no hit).
//!
//! All tests use real SQLite in a `TempDir` — no mocks, no in-memory stubs.

use tempfile::TempDir;
use vestige_core::{
    build_bundle, CandidateStatus, MemoryId, MemoryType, NewCandidate, NewCandidateSource,
    NewMemory, ProjectId, RejectionReason, SearchFilter,
};
use vestige_engine::{approve_candidate, propose_candidate, reject_candidate, ApprovalOverrides};
use vestige_store::Store;

// === TEST HELPERS ===

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_project(store: &mut Store, slug: &str) -> ProjectId {
    let project = ProjectId::from_slug(slug);
    store.ensure_project(&project, "Test", None, None).unwrap();
    project
}

fn seed_memory(store: &mut Store, project: &ProjectId, body: &str, ty: MemoryType) -> MemoryId {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: ty,
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

fn new_candidate(project: &ProjectId, body: &str, ty: MemoryType) -> NewCandidate {
    NewCandidate {
        project_id: project.clone(),
        proposed_type: ty,
        body: body.to_string(),
        rationale: None,
        title_override: None,
        importance: 0.6,
        confidence: 0.8,
        source: None,
        duplicate_of_memory_id: None,
        duplicate_of_candidate_id: None,
    }
}

// === APPROVE TESTS ===

#[test]
fn approve_candidate_creates_recallable_memory() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approve-recallable");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Use cargo workspaces for multi-crate monorepos.",
            MemoryType::Decision,
        ),
    )
    .unwrap();

    let outcome = approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    // Memory must be retrievable by ID.
    let memory = store.get_memory(&outcome.memory_id).unwrap();
    assert!(
        memory.is_some(),
        "approved candidate must create a retrievable memory"
    );

    // Memory must also be findable via FTS search.
    let hits = store
        .search_memories(&project, "cargo workspaces", &SearchFilter::default())
        .unwrap();
    assert!(
        !hits.is_empty(),
        "approved candidate must be findable via search_memories"
    );
    assert_eq!(hits[0].fetched.memory.id, outcome.memory_id);
}

#[test]
fn approve_candidate_writes_reverse_provenance() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approve-provenance");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "All public APIs must be documented with rustdoc.",
            MemoryType::Preference,
        ),
    )
    .unwrap();

    let outcome = approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    // There must be a memory_sources row with source_type='candidate' and
    // source_ref=<candidate_id> linking the new memory back to its origin.
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_sources
             WHERE memory_id = ?1 AND source_type = 'candidate' AND source_ref = ?2",
            rusqlite::params![outcome.memory_id.as_str(), proposed.candidate_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "reverse-provenance memory_sources row must be written"
    );
}

#[test]
fn approve_candidate_copies_original_sources() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approve-copy-sources");

    // Propose with a file source attached.
    let mut new_cand = new_candidate(
        &project,
        "Prefer newtype wrappers for domain IDs.",
        MemoryType::Preference,
    );
    new_cand.source = Some(NewCandidateSource {
        source_type: "file".to_string(),
        source_ref: Some("src/ids.rs:12".to_string()),
        source_content: Some("pub struct MemoryId(String);".to_string()),
    });

    let proposed = propose_candidate(&mut store, &project, new_cand).unwrap();

    let outcome = approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    // Count sources: expect 1 file source + 1 candidate provenance row = 2 total.
    let source_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_sources WHERE memory_id = ?1",
            rusqlite::params![outcome.memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        source_count, 2,
        "expected original file source + reverse-provenance source = 2 rows"
    );

    // The file source must be present.
    let file_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_sources
             WHERE memory_id = ?1 AND source_type = 'file' AND source_ref = 'src/ids.rs:12'",
            rusqlite::params![outcome.memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(file_count, 1, "original file source row must be copied");
}

#[test]
fn approve_candidate_marks_status_approved() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approve-status");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Integration tests run against real SQLite, never mocks.",
            MemoryType::Preference,
        ),
    )
    .unwrap();

    let outcome = approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    let candidate = store
        .get_candidate(&proposed.candidate_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        candidate.status,
        CandidateStatus::Approved,
        "candidate status must flip to Approved"
    );
    assert_eq!(
        candidate.approved_memory_id.as_ref(),
        Some(&outcome.memory_id),
        "approved_memory_id must point at the new memory"
    );
    assert!(candidate.reviewed_at.is_some(), "reviewed_at must be set");
}

#[test]
fn approve_candidate_pending_only() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approve-pending-only");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Feature flags must be cleaned up within one sprint.",
            MemoryType::Preference,
        ),
    )
    .unwrap();

    // First approval succeeds.
    approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    // Second approval must fail.
    let err = approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            vestige_engine::error::EngineError::CandidateNotPending { .. }
        ),
        "second approval must return CandidateNotPending, got: {err}"
    );
}

#[test]
fn approve_candidate_out_of_scope() {
    let (_tmp, mut store) = open_store();
    let proj_a = seed_project(&mut store, "scope-a-approve");
    let proj_b = seed_project(&mut store, "scope-b-approve");

    // Propose under project A.
    let proposed = propose_candidate(
        &mut store,
        &proj_a,
        new_candidate(
            &proj_a,
            "Decision scoped to project A.",
            MemoryType::Decision,
        ),
    )
    .unwrap();

    // Approve from project B must fail with OutOfScope.
    let err = approve_candidate(
        &mut store,
        &proj_b,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, vestige_engine::error::EngineError::OutOfScope),
        "cross-project approval must return OutOfScope, got: {err}"
    );
}

// === REJECT TESTS ===

#[test]
fn reject_duplicate_persists_link() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "reject-dup-link");

    let dup_mem_id = MemoryId::new();

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Use SQLite as the canonical storage backend.",
            MemoryType::Decision,
        ),
    )
    .unwrap();

    reject_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        RejectionReason::Duplicate,
        Some(dup_mem_id.clone()),
        None,
    )
    .unwrap();

    let candidate = store
        .get_candidate(&proposed.candidate_id)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.status, CandidateStatus::Rejected);
    assert_eq!(
        candidate.duplicate_of_memory_id.as_ref(),
        Some(&dup_mem_id),
        "duplicate_of_memory_id must be persisted"
    );
}

#[test]
fn reject_with_duplicate_link_but_non_duplicate_reason_validation_error() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "reject-validation");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Something worth capturing here.",
            MemoryType::Note,
        ),
    )
    .unwrap();

    let bogus_mem_id = MemoryId::new();
    let err = reject_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        RejectionReason::NotDurable, // non-Duplicate reason with duplicate_of set
        Some(bogus_mem_id),
        None,
    )
    .unwrap_err();

    assert!(
        matches!(err, vestige_engine::error::EngineError::Validation { .. }),
        "duplicate_of with non-Duplicate reason must return Validation error, got: {err}"
    );
}

// === PROPOSE / DEDUP TESTS ===

#[test]
fn propose_returns_similar_memories_when_dedup_hits() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "dedup-hit");

    // Seed an active memory containing the keyword "skills".
    seed_memory(
        &mut store,
        &project,
        "We install skills to both claude and agents targets.",
        MemoryType::Decision,
    );

    // Propose a candidate with the same type and overlapping keywords.
    let outcome = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "We will install skills to both targets for cross-agent support.",
            MemoryType::Decision,
        ),
    )
    .unwrap();

    assert!(
        !outcome.similar_memories.is_empty(),
        "dedup probe must surface the active memory as similar"
    );
}

#[test]
fn propose_returns_empty_similars_on_fresh_project() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "dedup-empty");

    let outcome = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "Use tokio for async runtimes in future services.",
            MemoryType::Decision,
        ),
    )
    .unwrap();

    assert!(
        outcome.similar_memories.is_empty(),
        "fresh project must return no similar_memories"
    );
    assert!(
        outcome.similar_candidates.is_empty(),
        "fresh project must return no similar_candidates"
    );
}

#[test]
fn propose_filters_similar_by_type() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "dedup-type-filter");

    // Seed a Note memory with the keyword "skills".
    seed_memory(
        &mut store,
        &project,
        "skills targets notes about configuration.",
        MemoryType::Note, // Note, not Decision
    );

    // Propose a Decision candidate with the same keyword.
    let outcome = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "skills targets decision for cross-agent deployment.",
            MemoryType::Decision, // different type
        ),
    )
    .unwrap();

    // Dedup probe is type-scoped — Note should not appear in Decision similars.
    assert!(
        outcome.similar_memories.is_empty(),
        "cross-type dedup must return no similar_memories (Note ≠ Decision)"
    );
}

// === SEARCH ISOLATION TESTS ===

#[test]
fn pending_candidate_invisible_to_recall_or_search() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "pending-invisible");

    // Propose a candidate — it stays pending.
    propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "uniquezephyrtoken pending memory must not leak.",
            MemoryType::Note,
        ),
    )
    .unwrap();

    // Memory search must return nothing.
    let hits = store
        .search_memories(&project, "uniquezephyrtoken", &SearchFilter::default())
        .unwrap();
    assert!(
        hits.is_empty(),
        "pending candidate must be invisible to search_memories"
    );
}

#[test]
fn approved_candidate_becomes_visible_in_search() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "approved-visible");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "uniquequasartoken approved memory should be searchable.",
            MemoryType::Note,
        ),
    )
    .unwrap();

    // Before approval: invisible.
    let before = store
        .search_memories(&project, "uniquequasartoken", &SearchFilter::default())
        .unwrap();
    assert!(
        before.is_empty(),
        "pending must be invisible before approval"
    );

    // After approval: visible.
    approve_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        ApprovalOverrides::default(),
    )
    .unwrap();

    let after = store
        .search_memories(&project, "uniquequasartoken", &SearchFilter::default())
        .unwrap();
    assert!(
        !after.is_empty(),
        "approved candidate must appear in search_memories"
    );
}

#[test]
fn rejected_candidate_invisible_to_recall_or_search() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "rejected-invisible");

    let proposed = propose_candidate(
        &mut store,
        &project,
        new_candidate(
            &project,
            "uniquepulsartoken rejected memory must stay hidden.",
            MemoryType::Note,
        ),
    )
    .unwrap();

    reject_candidate(
        &mut store,
        &project,
        &proposed.candidate_id,
        RejectionReason::Stale,
        None,
        None,
    )
    .unwrap();

    let hits = store
        .search_memories(&project, "uniquepulsartoken", &SearchFilter::default())
        .unwrap();
    assert!(
        hits.is_empty(),
        "rejected candidate must never appear in search_memories"
    );
}
