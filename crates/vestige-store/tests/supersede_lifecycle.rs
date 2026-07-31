//! Integration tests for `Store::supersede_memory` and the `superseded_by`
//! extension to `Store::restore_memory` (issue #131).
//!
//! Supersession reuses the existing `'deleted'` status — there is no new
//! `MemoryStatus` variant — plus the `superseded_by` column added by
//! migration 0007 (issue #128). These tests cover:
//!
//! 1. `supersede_memory` flips status/sets `superseded_by`/appends the
//!    `memory.superseded` event.
//! 2. The superseded memory drops out of search via the existing status
//!    trigger — proof #128's new columns didn't break trigger scoping.
//! 3. Superseding an already-deleted memory is an idempotent no-op (`false`),
//!    not an error — same semantics as `forget_memory`.
//! 4. `restore_memory` clears `superseded_by`, reactivates the old memory,
//!    and does not cascade-delete the superseding memory.

use tempfile::TempDir;
use vestige_core::{
    build_bundle, sanitize_fts_query, MemoryId, MemoryType, NewMemory, ProjectId, SearchFilter,
};
use vestige_store::Store;

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_memory(store: &mut Store, project: &ProjectId, body: &str) -> MemoryId {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: MemoryType::Decision,
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

#[test]
fn supersede_sets_status_and_link_and_appends_event() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-basic");
    store.ensure_project(&project, "P", None, None).unwrap();

    let old_id = seed_memory(&mut store, &project, "Use SQLite as the canonical store.");
    let new_id = seed_memory(
        &mut store,
        &project,
        "Use SQLite with WAL mode as the canonical store.",
    );

    let flipped = store.supersede_memory(&old_id, &new_id).unwrap();
    assert!(flipped, "supersede_memory must report the row was updated");

    let (status, deleted_at, superseded_by): (String, Option<String>, Option<String>) = store
        .connection()
        .query_row(
            "SELECT status, deleted_at, superseded_by FROM memories WHERE id = ?1",
            rusqlite::params![old_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "deleted");
    assert!(deleted_at.is_some(), "deleted_at must be set");
    assert_eq!(superseded_by.as_deref(), Some(new_id.as_str()));

    // Journal must carry a memory.superseded event on the old memory.
    let (event_type, payload_json): (String, Option<String>) = store
        .connection()
        .query_row(
            "SELECT event_type, payload_json FROM memory_events
             WHERE memory_id = ?1 AND event_type = 'memory.superseded'",
            rusqlite::params![old_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(event_type, "memory.superseded");
    let payload: serde_json::Value =
        serde_json::from_str(&payload_json.expect("payload_json present")).unwrap();
    assert_eq!(payload["memory_id"].as_str(), Some(old_id.as_str()));
    assert_eq!(payload["superseded_by"].as_str(), Some(new_id.as_str()));

    // New memory must be untouched by the supersede call.
    let new_status: String = store
        .connection()
        .query_row(
            "SELECT status FROM memories WHERE id = ?1",
            rusqlite::params![new_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_status, "active");
}

#[test]
fn superseded_memory_drops_out_of_search() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-fts");
    store.ensure_project(&project, "P", None, None).unwrap();

    let old_id = seed_memory(
        &mut store,
        &project,
        "Rust is the systems language of choice.",
    );
    let new_id = seed_memory(
        &mut store,
        &project,
        "Rust with edition 2021 is the systems language of choice.",
    );

    let q = sanitize_fts_query("systems language");
    let hits = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(
        hits.len(),
        2,
        "both memories should be findable before supersede"
    );

    assert!(store.supersede_memory(&old_id, &new_id).unwrap());

    let hits = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(
        hits.len(),
        1,
        "superseded memory must drop out of search via the status trigger"
    );
    assert_eq!(hits[0].fetched.memory.id, new_id);
}

#[test]
fn supersede_already_deleted_memory_is_idempotent_no_op() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-idempotent");
    store.ensure_project(&project, "P", None, None).unwrap();

    let old_id = seed_memory(
        &mut store,
        &project,
        "Deprecated approach to config loading.",
    );
    let new_id = seed_memory(&mut store, &project, "New approach to config loading.");

    assert!(store.forget_memory(&old_id).unwrap());

    // Superseding an already-deleted memory must not error — same `bool`
    // semantics as forget_memory's own idempotent-no-op behaviour.
    let flipped = store.supersede_memory(&old_id, &new_id).unwrap();
    assert!(!flipped, "superseding an already-deleted memory is a no-op");

    // superseded_by must remain unset — the no-op UPDATE never ran.
    let superseded_by: Option<String> = store
        .connection()
        .query_row(
            "SELECT superseded_by FROM memories WHERE id = ?1",
            rusqlite::params![old_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(superseded_by, None);

    // No memory.superseded event should have been appended.
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_events
             WHERE memory_id = ?1 AND event_type = 'memory.superseded'",
            rusqlite::params![old_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn restore_clears_link_reactivates_old_and_leaves_new_untouched() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-restore");
    store.ensure_project(&project, "P", None, None).unwrap();

    let old_id = seed_memory(&mut store, &project, "Old note about caching.");
    let new_id = seed_memory(&mut store, &project, "New note about caching, revised.");

    assert!(store.supersede_memory(&old_id, &new_id).unwrap());
    assert!(store.restore_memory(&old_id).unwrap());

    let (status, deleted_at, superseded_by): (String, Option<String>, Option<String>) = store
        .connection()
        .query_row(
            "SELECT status, deleted_at, superseded_by FROM memories WHERE id = ?1",
            rusqlite::params![old_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "active", "restored memory must be active again");
    assert_eq!(deleted_at, None);
    assert_eq!(superseded_by, None, "restore must clear the supersede link");

    // The superseding memory (B) must be untouched — no cascade delete.
    let new_status: String = store
        .connection()
        .query_row(
            "SELECT status FROM memories WHERE id = ?1",
            rusqlite::params![new_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_status, "active");
}

#[test]
fn restore_clears_null_superseded_by_as_no_op_for_plain_forget() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-plain-forget");
    store.ensure_project(&project, "P", None, None).unwrap();

    let id = seed_memory(&mut store, &project, "A note that gets plainly forgotten.");
    assert!(store.forget_memory(&id).unwrap());
    assert!(store.restore_memory(&id).unwrap());

    let (status, superseded_by): (String, Option<String>) = store
        .connection()
        .query_row(
            "SELECT status, superseded_by FROM memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "active");
    assert_eq!(superseded_by, None);
}

#[test]
fn find_superseded_memory_reverse_lookup_holds_both_directions() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("supersede-reverse-lookup");
    store.ensure_project(&project, "P", None, None).unwrap();

    let old_id = seed_memory(&mut store, &project, "First cut of the retry policy.");
    let new_id = seed_memory(&mut store, &project, "Revised retry policy with backoff.");

    // Before supersede: no lineage in either direction.
    assert_eq!(store.find_superseded_memory(&new_id).unwrap(), None);

    assert!(store.supersede_memory(&old_id, &new_id).unwrap());

    // Forward direction lives on the row itself.
    let fetched_old = store.get_memory(&old_id).unwrap().unwrap();
    assert_eq!(fetched_old.memory.superseded_by, Some(new_id.clone()));

    // Reverse direction via the new index-backed lookup.
    assert_eq!(
        store.find_superseded_memory(&new_id).unwrap(),
        Some(old_id.clone())
    );
    // The old memory never superseded anything itself.
    assert_eq!(store.find_superseded_memory(&old_id).unwrap(), None);
}
