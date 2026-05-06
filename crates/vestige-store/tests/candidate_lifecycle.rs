//! Cross-crate integration tests for candidate store invariants (V0.2).
//!
//! These tests exercise regressions that bite if a future migration or schema
//! change breaks the candidate lifecycle. They deliberately duplicate nothing
//! from the inline unit tests in `candidate_ops/`; the split is:
//!
//! - Inline tests: per-method happy/sad path, SQL round-trips.
//! - Here: multi-step workflows, FTS trigger correctness, audit-event payloads,
//!   cross-project scope isolation, and the 2 KiB source-content cap.
//!
//! All tests use real SQLite in a `TempDir` — no mocks.

use tempfile::TempDir;
use vestige_core::{
    build_candidate_bundle, CandidateId, CandidateStatus, MemoryType, NewCandidate,
    NewCandidateSource, ProjectId, RejectionReason, SOURCE_SNIPPET_MAX_BYTES,
};
use vestige_store::{CandidateFilter, Store};

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

fn record_candidate(store: &mut Store, project: &ProjectId, body: &str) -> CandidateId {
    let bundle = build_candidate_bundle(NewCandidate {
        project_id: project.clone(),
        proposed_type: MemoryType::Decision,
        body: body.to_string(),
        rationale: None,
        title_override: None,
        importance: 0.6,
        confidence: 0.8,
        source: None,
        duplicate_of_memory_id: None,
        duplicate_of_candidate_id: None,
    })
    .unwrap();
    let id = bundle.id.clone();
    store.record_candidate(&bundle).unwrap();
    id
}

fn count_events(store: &Store, event_type: &str) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_events WHERE event_type = ?1",
            rusqlite::params![event_type],
            |r| r.get(0),
        )
        .unwrap()
}

// === TESTS ===

#[test]
fn record_candidate_round_trip() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "round-trip");

    let cand_id = record_candidate(&mut store, &project, "Use Rust for all systems work.");

    // list_candidates returns it.
    let list = store
        .list_candidates(&project, &CandidateFilter::default())
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, cand_id);
    assert_eq!(list[0].status, CandidateStatus::Pending);

    // get_candidate returns the full row.
    let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
    assert_eq!(candidate.id, cand_id);
    assert_eq!(candidate.proposed_type, MemoryType::Decision);
    assert!(!candidate.title.is_empty());
    assert!(!candidate.full_body.is_empty());
}

#[test]
fn pending_candidates_excluded_from_memory_search() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "pending-isolation");

    // Record a candidate with a distinctive keyword.
    record_candidate(
        &mut store,
        &project,
        "quirkyxyztoken must stay out of memory recall.",
    );

    // search_memories must return nothing — candidates are a separate table.
    let hits = store
        .search_memories(
            &project,
            "quirkyxyztoken",
            &vestige_core::SearchFilter::default(),
        )
        .unwrap();
    assert!(
        hits.is_empty(),
        "pending candidate must not leak into memory_fts search"
    );
}

#[test]
fn mark_candidate_approved_double_call_fails() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "double-approve");

    let cand_id = record_candidate(&mut store, &project, "Approve this candidate once.");
    let mem_id = vestige_core::MemoryId::new();
    store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

    let second_mem = vestige_core::MemoryId::new();
    let err = store
        .mark_candidate_approved(&cand_id, &second_mem)
        .unwrap_err();
    assert!(
        matches!(&err, vestige_store::StoreError::Corruption(msg) if msg.contains("CandidateNotPending")),
        "second approval must fail with CandidateNotPending, got: {err}"
    );
}

#[test]
fn mark_candidate_rejected_persists_reason_and_duplicate_link() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "reject-reason");

    let cand_id = record_candidate(&mut store, &project, "A candidate to be rejected.");
    let dup_mem_id = vestige_core::MemoryId::new();

    store
        .mark_candidate_rejected(
            &cand_id,
            &RejectionReason::Duplicate,
            Some(&dup_mem_id),
            Some("Already captured."),
        )
        .unwrap();

    let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
    assert_eq!(candidate.status, CandidateStatus::Rejected);
    assert_eq!(candidate.rejection_reason, Some(RejectionReason::Duplicate));
    assert_eq!(candidate.duplicate_of_memory_id.as_ref(), Some(&dup_mem_id));
    assert_eq!(candidate.review_note.as_deref(), Some("Already captured."));
    assert!(candidate.reviewed_at.is_some());
}

#[test]
fn reject_after_approve_fails() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "reject-after-approve");

    let cand_id = record_candidate(&mut store, &project, "Approve then try to reject.");
    let mem_id = vestige_core::MemoryId::new();
    store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

    let err = store
        .mark_candidate_rejected(&cand_id, &RejectionReason::Wrong, None, None)
        .unwrap_err();
    assert!(
        matches!(&err, vestige_store::StoreError::Corruption(msg) if msg.contains("CandidateNotPending")),
        "reject after approve must fail with CandidateNotPending, got: {err}"
    );
}

#[test]
fn project_scope_isolation() {
    let (_tmp, mut store) = open_store();
    let proj_a = seed_project(&mut store, "scope-proj-a");
    let proj_b = seed_project(&mut store, "scope-proj-b");

    let cand_a = record_candidate(&mut store, &proj_a, "Project A decision.");
    record_candidate(&mut store, &proj_b, "Project B decision.");

    // list_candidates(proj_a) returns exactly project A's candidate.
    let list_a = store
        .list_candidates(&proj_a, &CandidateFilter::default())
        .unwrap();
    assert_eq!(list_a.len(), 1, "proj_a must see exactly 1 candidate");
    assert_eq!(list_a[0].id, cand_a);

    // list_candidates(proj_b) returns exactly project B's candidate.
    let list_b = store
        .list_candidates(&proj_b, &CandidateFilter::default())
        .unwrap();
    assert_eq!(list_b.len(), 1, "proj_b must see exactly 1 candidate");
    assert_ne!(list_b[0].id, cand_a);
}

#[test]
fn fts_dedup_excludes_approved() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "fts-approved");

    let cand_id = record_candidate(
        &mut store,
        &project,
        "rustlang ownership borrow checker guarantees memory safety.",
    );

    // Pending → FTS hit.
    let hits = store
        .search_candidates_lexical(&project, "ownership borrow", &CandidateFilter::default())
        .unwrap();
    assert_eq!(hits.len(), 1, "pending candidate must appear in FTS");

    // Approve → trigger removes FTS row.
    let mem_id = vestige_core::MemoryId::new();
    store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

    let hits_after = store
        .search_candidates_lexical(&project, "ownership borrow", &CandidateFilter::default())
        .unwrap();
    assert!(
        hits_after.is_empty(),
        "approved candidate must leave candidate_fts (trigger must fire)"
    );
}

#[test]
fn fts_dedup_excludes_rejected() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "fts-rejected");

    let cand_id = record_candidate(
        &mut store,
        &project,
        "temporaryxyz prototype spike for exploration only.",
    );

    let hits = store
        .search_candidates_lexical(&project, "prototype spike", &CandidateFilter::default())
        .unwrap();
    assert_eq!(hits.len(), 1, "pending candidate must appear in FTS");

    // Reject → trigger removes FTS row.
    store
        .mark_candidate_rejected(&cand_id, &RejectionReason::NotDurable, None, None)
        .unwrap();

    let hits_after = store
        .search_candidates_lexical(&project, "prototype spike", &CandidateFilter::default())
        .unwrap();
    assert!(
        hits_after.is_empty(),
        "rejected candidate must leave candidate_fts (trigger must fire)"
    );
}

#[test]
fn record_candidate_emits_proposed_event() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "event-proposed");

    record_candidate(&mut store, &project, "Decision worth capturing.");

    assert_eq!(
        count_events(&store, "candidate.proposed"),
        1,
        "must emit exactly one candidate.proposed event"
    );
}

#[test]
fn mark_candidate_approved_emits_audit_event() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "event-approved");

    let cand_id = record_candidate(&mut store, &project, "Candidate to approve.");
    let mem_id = vestige_core::MemoryId::new();
    store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

    assert_eq!(
        count_events(&store, "candidate.approved"),
        1,
        "must emit exactly one candidate.approved event"
    );

    // Payload must contain both IDs.
    let payload: String = store
        .connection()
        .query_row(
            "SELECT payload_json FROM memory_events WHERE event_type = 'candidate.approved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(json["candidate_id"].as_str(), Some(cand_id.as_str()));
    assert_eq!(json["memory_id"].as_str(), Some(mem_id.as_str()));
}

#[test]
fn mark_candidate_rejected_emits_audit_event() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "event-rejected");

    let cand_id = record_candidate(&mut store, &project, "Candidate to reject.");
    store
        .mark_candidate_rejected(&cand_id, &RejectionReason::TooNoisy, None, None)
        .unwrap();

    assert_eq!(
        count_events(&store, "candidate.rejected"),
        1,
        "must emit exactly one candidate.rejected event"
    );

    // Payload must contain candidate_id and reason.
    let payload: String = store
        .connection()
        .query_row(
            "SELECT payload_json FROM memory_events WHERE event_type = 'candidate.rejected'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(json["candidate_id"].as_str(), Some(cand_id.as_str()));
    assert_eq!(json["reason"].as_str(), Some("too_noisy"));
}

#[test]
fn include_rejected_filter_works() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "include-rejected");

    let cand_id = record_candidate(&mut store, &project, "Candidate that will be rejected.");
    store
        .mark_candidate_rejected(&cand_id, &RejectionReason::Stale, None, None)
        .unwrap();

    // Default filter: only pending — returns 0.
    let default_list = store
        .list_candidates(&project, &CandidateFilter::default())
        .unwrap();
    assert_eq!(
        default_list.len(),
        0,
        "default list must exclude rejected candidates"
    );

    // include_rejected = true: returns 1.
    let inclusive_list = store
        .list_candidates(
            &project,
            &CandidateFilter {
                include_rejected: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        inclusive_list.len(),
        1,
        "include_rejected=true must surface rejected candidates"
    );
    assert_eq!(inclusive_list[0].status, CandidateStatus::Rejected);
}

#[test]
fn source_content_truncation() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "src-truncation");

    // Build a source content payload larger than the 2 KiB cap.
    let oversized = "a".repeat(SOURCE_SNIPPET_MAX_BYTES + 500);

    let bundle = build_candidate_bundle(NewCandidate {
        project_id: project.clone(),
        proposed_type: MemoryType::Note,
        body: "Note with a very large source attachment.".to_string(),
        rationale: None,
        title_override: None,
        importance: 0.5,
        confidence: 0.7,
        source: Some(NewCandidateSource {
            source_type: "file".to_string(),
            source_ref: Some("large_file.rs".to_string()),
            source_content: Some(oversized),
        }),
        duplicate_of_memory_id: None,
        duplicate_of_candidate_id: None,
    })
    .unwrap();

    let cand_id = bundle.id.clone();
    store.record_candidate(&bundle).unwrap();

    let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
    assert_eq!(candidate.sources.len(), 1, "source row must be present");

    let content = candidate.sources[0]
        .source_content
        .as_deref()
        .expect("source_content must be present");

    assert!(
        content.len() <= SOURCE_SNIPPET_MAX_BYTES,
        "source_content must be truncated to at most {} bytes, got {} bytes",
        SOURCE_SNIPPET_MAX_BYTES,
        content.len()
    );
    // Content must end on a UTF-8 codepoint boundary (no panic on `.chars()`).
    let _ = content.chars().count();
}
