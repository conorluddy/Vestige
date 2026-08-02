//! Integration tests for `Store::revise_memory` (issue #130).
//!
//! `memory_representations` has `UNIQUE(memory_id, representation_type)`, so
//! revision must UPDATE the four existing rows rather than INSERT new ones.
//! That UPDATE is also the load-bearing mechanism here: it activates two
//! shipped-but-previously-dead triggers — `memory_repr_after_update`
//! (0002_fts.sql, resyncs FTS) and `embedding_repr_content_changed`
//! (0003_embeddings.sql, marks embeddings stale on `content_hash` change).
//! The tests below prove both fired, not just that the row count held.

use std::collections::BTreeMap;
use std::str::FromStr;

use tempfile::TempDir;
use vestige_core::{
    build_bundle, sanitize_fts_query, MemoryId, MemoryType, NewMemory, ProjectId,
    RepresentationDepth, SearchFilter,
};
use vestige_store::{NewEmbedding, Store, StoreError};

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_memory(store: &mut Store, project: &ProjectId, body: &str) -> MemoryId {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: MemoryType::Note,
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

/// `(representation_id, content)` per `representation_type`, for diffing
/// before/after a revision.
fn representation_rows(store: &Store, id: &MemoryId) -> BTreeMap<String, (String, String)> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT representation_type, id, content
             FROM memory_representations
             WHERE memory_id = ?1",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![id.as_str()], |row| {
        let depth: String = row.get(0)?;
        let rep_id: String = row.get(1)?;
        let content: String = row.get(2)?;
        Ok((depth, (rep_id, content)))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn seed_project(store: &mut Store, slug: &str) -> ProjectId {
    let project = ProjectId::from_slug(slug);
    store.ensure_project(&project, slug, None, None).unwrap();
    project
}

// ── representations: UPDATE, never INSERT ──────────────────────────────────

#[test]
fn revise_updates_all_four_representations_in_place() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-reps");
    let id = seed_memory(&mut store, &project, "The garbanzo pipeline runs nightly.");

    let before = representation_rows(&store, &id);
    assert_eq!(before.len(), 4, "record_memory always seeds four depths");

    store
        .revise_memory(&id, "The falafel pipeline runs hourly now.")
        .unwrap();

    let after = representation_rows(&store, &id);
    assert_eq!(after.len(), 4, "revision must not change the row count");

    for depth in ["one_liner", "summary", "compressed", "full"] {
        let (id_before, content_before) = &before[depth];
        let (id_after, content_after) = &after[depth];
        assert_eq!(
            id_before, id_after,
            "`{depth}` row id changed — revision must UPDATE, not replace"
        );
        assert_ne!(
            content_before, content_after,
            "`{depth}` content must reflect the new body"
        );
        assert!(content_after.contains("falafel"));
    }
}

#[test]
fn revise_returns_prior_and_new_one_liners() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-outcome");
    let id = seed_memory(&mut store, &project, "Original one-liner body.");

    let outcome = store
        .revise_memory(&id, "Replacement one-liner body.")
        .unwrap();

    assert!(outcome.prior_one_liner.contains("Original"));
    assert!(outcome.new_one_liner.contains("Replacement"));
}

// ── FTS resync (`memory_repr_after_update`) ─────────────────────────────────

#[test]
fn revise_drops_old_tokens_and_indexes_new_ones() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-fts");
    let id = seed_memory(&mut store, &project, "The garbanzo pipeline runs nightly.");

    let old_query = sanitize_fts_query("garbanzo");
    let new_query = sanitize_fts_query("falafel");

    assert_eq!(
        store
            .search_memories(&project, &old_query, &SearchFilter::default())
            .unwrap()
            .len(),
        1,
        "old token must be indexed before revision"
    );

    store
        .revise_memory(&id, "The falafel pipeline runs hourly now.")
        .unwrap();

    let old_hits = store
        .search_memories(&project, &old_query, &SearchFilter::default())
        .unwrap();
    assert!(
        old_hits.is_empty(),
        "a token unique to the old body must not match after revision"
    );

    let new_hits = store
        .search_memories(&project, &new_query, &SearchFilter::default())
        .unwrap();
    assert_eq!(new_hits.len(), 1, "the new token must be indexed and match");
    assert_eq!(new_hits[0].fetched.memory.id, id);
}

// ── embedding staleness (`embedding_repr_content_changed`) ─────────────────

#[test]
fn revise_marks_every_embedding_for_the_memory_stale() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-embed");
    let id = seed_memory(&mut store, &project, "Embed this before revising it.");

    // Embed all four representations with the deterministic `fake`-style
    // vector inputs a real provider would produce — otherwise the staleness
    // assertion below would be vacuously true (no rows to mark stale).
    for depth in [
        RepresentationDepth::OneLiner,
        RepresentationDepth::Summary,
        RepresentationDepth::Compressed,
        RepresentationDepth::Full,
    ] {
        let representation_id = store.repr_id_for_depth(&id, depth).unwrap().unwrap();
        store
            .record_embedding(&NewEmbedding {
                memory_id: &id,
                representation_id: &representation_id,
                representation_type: depth.as_str(),
                provider: "fake",
                model: "fake-deterministic",
                vector: &[0.1, 0.2, 0.3, 0.4],
            })
            .unwrap();
    }

    let active_before: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = ?1 AND status = 'active'",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(active_before, 4, "all four embeddings must start active");

    store
        .revise_memory(&id, "Content changed; embeddings should go stale.")
        .unwrap();

    let stale_after: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = ?1 AND status = 'stale'",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stale_after, 4,
        "every embedding for the memory must flip to stale after revision"
    );
}

// ── identity, timestamps, and usage counters ────────────────────────────────

#[test]
fn revise_preserves_identity_and_usage_but_advances_updated_at() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-identity");
    let id = seed_memory(&mut store, &project, "A memory with some usage history.");

    // Give it usage history that must survive the revision untouched.
    store.bump_recall_stats(std::slice::from_ref(&id)).unwrap();
    store.bump_recall_stats(std::slice::from_ref(&id)).unwrap();
    store.bump_expand_stats(&id).unwrap();

    let before = store.get_memory(&id).unwrap().unwrap().memory;
    assert_eq!(before.recall_count, 2);
    assert_eq!(before.expand_count, 1);
    assert_eq!(
        before.created_at, before.updated_at,
        "a freshly recorded, unrevised memory has updated_at == created_at"
    );

    store
        .revise_memory(&id, "A revised body entirely.")
        .unwrap();

    let after = store.get_memory(&id).unwrap().unwrap().memory;
    assert_eq!(after.id, before.id);
    assert_eq!(
        after.created_at, before.created_at,
        "created_at must not change"
    );
    assert!(
        after.updated_at > before.updated_at,
        "updated_at must advance past its create-time value"
    );
    assert_eq!(
        after.recall_count, 2,
        "revision must not reset recall_count (issue #128 usage history)"
    );
    assert_eq!(
        after.expand_count, 1,
        "revision must not reset expand_count (issue #128 usage history)"
    );
    assert_eq!(
        after.last_recalled_at, before.last_recalled_at,
        "revision must not clear last_recalled_at — the UPDATE never names that column"
    );
}

// ── journal event ────────────────────────────────────────────────────────────

#[test]
fn revise_appends_memory_revised_event_with_full_prior_body() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-event");
    let id = seed_memory(&mut store, &project, "Original body for the journal test.");

    store
        .revise_memory(&id, "Revised body for the journal test.")
        .unwrap();

    let events = store.fetch_memory_events(&id).unwrap();
    let revised = events
        .iter()
        .find(|e| e.event_type == "memory.revised")
        .expect("memory.revised event must be journaled and visible via `vestige why`");

    let payload: serde_json::Value =
        serde_json::from_str(revised.payload_json.as_deref().unwrap()).unwrap();
    assert_eq!(payload["memory_id"], id.as_str());
    assert_eq!(payload["prior_body"], "Original body for the journal test.");
    assert!(payload["prior_one_liner"]
        .as_str()
        .unwrap()
        .contains("Original"));
    assert!(!payload["prior_content_hash"].as_str().unwrap().is_empty());
    assert!(!payload["new_content_hash"].as_str().unwrap().is_empty());
    assert_ne!(payload["prior_content_hash"], payload["new_content_hash"]);
}

// ── guards ───────────────────────────────────────────────────────────────────

#[test]
fn revise_deleted_memory_errors_instead_of_silently_succeeding() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-deleted");
    let id = seed_memory(&mut store, &project, "About to be forgotten.");
    assert!(store.forget_memory(&id).unwrap());

    let err = store.revise_memory(&id, "Should never land.").unwrap_err();
    assert!(
        matches!(err, StoreError::Validation(_)),
        "a non-active memory is a caller-fixable precondition, not corruption; got: {err}"
    );
}

#[test]
fn revise_unknown_id_errors_with_not_found() {
    let (_tmp, mut store) = open_store();
    let unknown = MemoryId::new();

    let err = store.revise_memory(&unknown, "Anything.").unwrap_err();
    assert!(
        matches!(err, StoreError::NotFound(_)),
        "expected NotFound for an ID that never existed, got: {err}"
    );
}

#[test]
fn revise_empty_body_errors_with_validation() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "revise-empty");
    let id = seed_memory(&mut store, &project, "Has a body for now.");

    let err = store.revise_memory(&id, "   \n\t").unwrap_err();
    assert!(
        matches!(err, StoreError::Validation(_)),
        "expected a validation-shaped error, got: {err}"
    );

    // And the original body must be untouched — the guard must fire before
    // any representation row is touched.
    let after = representation_rows(&store, &id);
    assert!(after["full"].1.contains("Has a body for now"));
}

#[test]
fn revise_unparsable_id_is_rejected_by_the_caller_boundary() {
    // MemoryId::from_str rejects the wrong prefix — this is a core-layer
    // invariant, exercised here only to document that revise_memory takes an
    // already-typed MemoryId and never a bare String.
    assert!(MemoryId::from_str("not_a_memory_id").is_err());
}
