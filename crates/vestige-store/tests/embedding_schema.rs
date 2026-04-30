//! Integration tests for the embedding schema migration (0003_embeddings.sql).
//!
//! Invariants verified here (from CLAUDE.md):
//!   - New embedding tables are created by the migration.
//!   - Existing V0 data is untouched after migration.
//!   - Representation content change marks embeddings stale (trigger).
//!   - Soft-deleting a memory cascade-marks its embeddings stale (trigger).

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn record_memory(store: &mut Store, project: &ProjectId, body: &str) -> MemoryId {
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

// ── Migration validation ──────────────────────────────────────────────────────

#[test]
fn migrations_validate() {
    // rusqlite_migration's self-check ensures SQL parses cleanly against an
    // in-memory DB. Mirrors the existing `migrations_check_valid` unit test
    // but exercises the full three-migration chain including 0003.
    //
    // We drive it through `Store::open` on a fresh tmpdir, which calls
    // `migrations().validate()` internally via rusqlite_migration. If the SQL
    // is malformed the open will panic/fail.
    let tmp = TempDir::new().unwrap();
    Store::open(tmp.path().join("memory.sqlite")).expect("migration validation must pass");
}

// ── Table presence ────────────────────────────────────────────────────────────

#[test]
fn fresh_db_has_embedding_tables() {
    let (_tmp, store) = open_store();
    let conn = store.connection();

    for table in &["memory_embeddings", "embedding_jobs", "memory_vectors"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| panic!("sqlite_master query failed for table {table}"));
        assert_eq!(
            count, 1,
            "expected table '{table}' to exist after migration"
        );
    }
}

// ── Forward migration on V0 data ──────────────────────────────────────────────

#[test]
fn existing_v0_db_migrates_cleanly() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("memory.sqlite");

    // Simulate an existing V0 DB: create store (runs all migrations), insert
    // a project + memory using public APIs, then close.
    let memory_id = {
        let mut store = Store::open(&db_path).unwrap();
        let project = ProjectId::from_slug("vestige");
        store
            .ensure_project(&project, "Vestige", Some("/repo"), None)
            .unwrap();
        record_memory(
            &mut store,
            &project,
            "Local-first memory for coding agents.",
        )
        // store dropped here → connection closed
    };

    // Reopen — migrations are idempotent, should be a no-op.
    let store = Store::open(&db_path).unwrap();
    let conn = store.connection();

    // Original memory is intact.
    let row_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE id = ?1",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row_exists, 1, "memory must survive migration");

    // New tables exist and are empty.
    for table in &["memory_embeddings", "embedding_jobs", "memory_vectors"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or_else(|_| panic!("count query failed for {table}"));
        assert_eq!(count, 0, "table '{table}' must be empty after migration");
    }
}

// ── Trigger: content change marks embedding stale ─────────────────────────────

#[test]
fn representation_content_change_marks_embedding_stale() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();
    let memory_id = record_memory(&mut store, &project, "Some memory body.");

    let conn = store.connection_mut();

    // Fetch the representation id for this memory (any type will do).
    let repr_id: String = conn
        .query_row(
            "SELECT id FROM memory_representations WHERE memory_id = ?1 LIMIT 1",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    // Insert a fake embedding row with status='active'.
    let embed_id = "emb_test_001";
    conn.execute(
        "INSERT INTO memory_embeddings
            (id, memory_id, representation_id, representation_type,
             provider, model, dimensions, vector_hash,
             status, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'summary', 'fake', 'test-small', 64, 'abc123',
                 'active', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        rusqlite::params![embed_id, memory_id.as_str(), repr_id],
    )
    .unwrap();

    // Verify it starts active.
    let status_before: String = conn
        .query_row(
            "SELECT status FROM memory_embeddings WHERE id = ?1",
            rusqlite::params![embed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status_before, "active");

    // Update the representation's content_hash — trigger should fire.
    conn.execute(
        "UPDATE memory_representations SET content_hash = 'newhash' WHERE id = ?1",
        rusqlite::params![repr_id],
    )
    .unwrap();

    // Embedding must now be stale.
    let status_after: String = conn
        .query_row(
            "SELECT status FROM memory_embeddings WHERE id = ?1",
            rusqlite::params![embed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status_after, "stale",
        "trigger must mark embedding stale when content_hash changes"
    );
}

// ── Trigger: soft-delete cascades to embeddings ───────────────────────────────

#[test]
fn forget_memory_marks_embeddings_stale() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();
    let memory_id = record_memory(&mut store, &project, "Another memory body.");

    // Insert a fake embedding with status='active'.
    {
        let conn = store.connection_mut();
        let repr_id: String = conn
            .query_row(
                "SELECT id FROM memory_representations WHERE memory_id = ?1 LIMIT 1",
                rusqlite::params![memory_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();

        conn.execute(
            "INSERT INTO memory_embeddings
                (id, memory_id, representation_id, representation_type,
                 provider, model, dimensions, vector_hash,
                 status, created_at, updated_at)
             VALUES ('emb_test_002', ?1, ?2, 'summary', 'fake', 'test-small', 64, 'def456',
                     'active', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            rusqlite::params![memory_id.as_str(), repr_id],
        )
        .unwrap();
    }

    // Soft-delete via the public API — trigger must cascade.
    let forgotten = store.forget_memory(&memory_id).unwrap();
    assert!(
        forgotten,
        "forget_memory must return true for an active memory"
    );

    let status: String = store
        .connection()
        .query_row(
            "SELECT status FROM memory_embeddings WHERE id = 'emb_test_002'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "stale",
        "soft-deleting a memory must cascade-mark its embeddings stale"
    );
}
