//! Integration tests for the usage counters added in migration 0007 (#128).
//!
//! The load-bearing test here is [`counter_bump_does_not_fire_status_triggers`]:
//! every trigger on `memories` is scoped `AFTER UPDATE OF status`, so a
//! counter-only UPDATE must leave the FTS index and embedding staleness alone.
//! That invariant is asserted in prose on `bump_recall_stats` — this proves it.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_store::{NewEmbedding, Store};

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

fn counters(store: &Store, id: &MemoryId) -> (i64, i64, Option<String>) {
    let memory = store.get_memory(id).unwrap().unwrap().memory;
    (
        memory.recall_count,
        memory.expand_count,
        memory.last_recalled_at.map(|t| t.to_string()),
    )
}

// ── recall ────────────────────────────────────────────────────────────────────

#[test]
fn bump_recall_stats_increments_once_per_call() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("recall");
    store
        .ensure_project(&project, "Recall", None, None)
        .unwrap();
    let id = seed_memory(&mut store, &project, "Prefer hybrid recall by default.");

    let (recall_count, _, last_recalled_at) = counters(&store, &id);
    assert_eq!(
        recall_count, 0,
        "a freshly captured memory has never been recalled"
    );
    assert!(last_recalled_at.is_none());

    store.bump_recall_stats(std::slice::from_ref(&id)).unwrap();

    let (recall_count, expand_count, last_recalled_at) = counters(&store, &id);
    assert_eq!(recall_count, 1);
    assert_eq!(expand_count, 0, "recall must not touch expand_count");
    assert!(
        last_recalled_at.is_some(),
        "first recall must stamp last_recalled_at"
    );

    store.bump_recall_stats(std::slice::from_ref(&id)).unwrap();
    assert_eq!(counters(&store, &id).0, 2, "second call increments to 2");
}

#[test]
fn bump_recall_stats_bumps_every_id_in_the_batch() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("batch");
    store.ensure_project(&project, "Batch", None, None).unwrap();
    let first = seed_memory(&mut store, &project, "First memory body.");
    let second = seed_memory(&mut store, &project, "Second memory body.");
    let third = seed_memory(&mut store, &project, "Third memory body.");

    store
        .bump_recall_stats(&[first.clone(), second.clone(), third.clone()])
        .unwrap();

    for id in [&first, &second, &third] {
        assert_eq!(counters(&store, id).0, 1, "every id in the batch is bumped");
    }
}

#[test]
fn bump_recall_stats_on_empty_slice_is_a_no_op() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("empty");
    store.ensure_project(&project, "Empty", None, None).unwrap();
    let id = seed_memory(&mut store, &project, "Untouched memory.");

    store.bump_recall_stats(&[]).unwrap();

    assert_eq!(counters(&store, &id).0, 0);
}

// ── expand ────────────────────────────────────────────────────────────────────

#[test]
fn bump_expand_stats_increments_expand_count_only() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("expand");
    store
        .ensure_project(&project, "Expand", None, None)
        .unwrap();
    let id = seed_memory(&mut store, &project, "A memory worth expanding.");

    store.bump_expand_stats(&id).unwrap();

    let (recall_count, expand_count, last_recalled_at) = counters(&store, &id);
    assert_eq!(expand_count, 1);
    assert_eq!(recall_count, 0, "expand must not touch recall_count");
    assert!(
        last_recalled_at.is_none(),
        "expand must not stamp last_recalled_at — expansion is a distinct signal"
    );

    store.bump_expand_stats(&id).unwrap();
    assert_eq!(counters(&store, &id).1, 2);
}

// ── trigger inertness ─────────────────────────────────────────────────────────

#[test]
fn counter_bump_does_not_fire_status_triggers() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("triggers");
    store
        .ensure_project(&project, "Triggers", None, None)
        .unwrap();
    let id = seed_memory(&mut store, &project, "Embedded and indexed memory body.");

    // Give the memory an active embedding, so a spuriously-fired
    // `embedding_memory_soft_deleted` trigger would flip it to 'stale'.
    let representation_id = store
        .repr_id_for_depth(&id, RepresentationDepth::OneLiner)
        .unwrap()
        .expect("build_bundle always produces a one_liner representation");
    store
        .record_embedding(&NewEmbedding {
            memory_id: &id,
            representation_id: &representation_id,
            representation_type: "one_liner",
            provider: "fake",
            model: "fake-deterministic",
            vector: &[0.1, 0.2, 0.3, 0.4],
        })
        .unwrap();

    let embedding_status_before: String = store
        .connection()
        .query_row(
            "SELECT status FROM memory_embeddings WHERE memory_id = ?1",
            rusqlite::params![id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let fts_rows_before: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_fts WHERE memory_id = ?1",
            rusqlite::params![id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(embedding_status_before, "active");
    assert!(
        fts_rows_before > 0,
        "representations must be indexed in FTS"
    );

    store.bump_recall_stats(std::slice::from_ref(&id)).unwrap();
    store.bump_expand_stats(&id).unwrap();

    let embedding_status_after: String = store
        .connection()
        .query_row(
            "SELECT status FROM memory_embeddings WHERE memory_id = ?1",
            rusqlite::params![id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let fts_rows_after: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_fts WHERE memory_id = ?1",
            rusqlite::params![id.as_str()],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        embedding_status_after, "active",
        "counter bump must not mark embeddings stale — the UPDATE never SETs status"
    );
    assert_eq!(
        fts_rows_after, fts_rows_before,
        "counter bump must not disturb the FTS index"
    );

    // And the counters did land, so the test is not passing vacuously.
    let (recall_count, expand_count, _) = counters(&store, &id);
    assert_eq!((recall_count, expand_count), (1, 1));
}
