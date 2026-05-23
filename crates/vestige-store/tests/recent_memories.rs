//! Integration tests for `Store::recent_memories_by_created_at`.
//!
//! Invariants verified:
//!   1. Results are ordered by `created_at DESC`.
//!   2. Soft-deleted memories are excluded.
//!   3. The `limit` parameter is respected.
//!   4. Project-scope boundary: only memories from the requested project are returned.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

/// Insert a memory via the public API and return its id.
fn record(store: &mut Store, project: &ProjectId, body: &str) -> MemoryId {
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

/// Overwrite `created_at` on an existing memory row so tests can control sort order.
fn set_created_at(store: &Store, id: &MemoryId, created_at: &str) {
    store
        .connection()
        .execute(
            "UPDATE memories SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![created_at, id.as_str()],
        )
        .unwrap();
}

#[test]
fn returns_memories_in_created_at_desc_order() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    let id_a = record(&mut store, &project, "oldest memory");
    let id_b = record(&mut store, &project, "middle memory");
    let id_c = record(&mut store, &project, "newest memory");

    // Pin exact timestamps so order is deterministic regardless of wall-clock.
    set_created_at(&store, &id_a, "2025-01-01T00:00:00Z");
    set_created_at(&store, &id_b, "2025-06-01T00:00:00Z");
    set_created_at(&store, &id_c, "2025-12-01T00:00:00Z");

    let results = store.recent_memories_by_created_at(&project, 10).unwrap();

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].id, id_c, "newest must be first");
    assert_eq!(results[1].id, id_b, "middle must be second");
    assert_eq!(results[2].id, id_a, "oldest must be last");
}

#[test]
fn excludes_soft_deleted_memories() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    let id_active = record(&mut store, &project, "active memory");
    let id_deleted = record(&mut store, &project, "deleted memory");

    store.forget_memory(&id_deleted).unwrap();

    let results = store.recent_memories_by_created_at(&project, 10).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, id_active);
}

#[test]
fn respects_limit() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    for i in 0..5 {
        record(&mut store, &project, &format!("memory {i}"));
    }

    let results = store.recent_memories_by_created_at(&project, 3).unwrap();

    assert_eq!(results.len(), 3, "limit must be respected");
}

#[test]
fn scopes_to_project() {
    let (_tmp, mut store) = open_store();
    let project_a = ProjectId::from_slug("a");
    let project_b = ProjectId::from_slug("b");
    store.ensure_project(&project_a, "A", None, None).unwrap();
    store.ensure_project(&project_b, "B", None, None).unwrap();

    let id_a = record(&mut store, &project_a, "memory in project A");
    record(&mut store, &project_b, "memory in project B");

    let results_a = store.recent_memories_by_created_at(&project_a, 10).unwrap();

    assert_eq!(results_a.len(), 1, "only project A's memory must appear");
    assert_eq!(results_a[0].id, id_a);
    assert_eq!(
        results_a[0].project_id, project_a,
        "returned memory must belong to project A"
    );

    let results_b = store.recent_memories_by_created_at(&project_b, 10).unwrap();

    assert_eq!(results_b.len(), 1, "only project B's memory must appear");
    assert_ne!(
        results_b[0].id, id_a,
        "project A's memory must not leak into project B results"
    );
}
