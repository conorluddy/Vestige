//! Integration tests for the three new Store embedding-helper methods.
//!
//! Covers: `repr_id_for_depth`, `has_active_embedding`, `record_failed_embedding_job`.

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

// ── repr_id_for_depth ─────────────────────────────────────────────────────────

#[test]
fn repr_id_for_depth_returns_some_for_existing_representation() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test");
    store.ensure_project(&project, "Test", None, None).unwrap();
    let memory_id = seed_memory(&mut store, &project, "Hello world.");

    // build_bundle always produces a OneLiner representation.
    let result = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::OneLiner)
        .unwrap();
    assert!(
        result.is_some(),
        "expected Some(id) for OneLiner representation"
    );
    let id = result.unwrap();
    assert!(
        id.starts_with("rep_"),
        "representation id must start with rep_"
    );
}

#[test]
fn repr_id_for_depth_returns_none_for_absent_depth() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test2");
    store.ensure_project(&project, "Test2", None, None).unwrap();
    let memory_id = seed_memory(&mut store, &project, "Another memory.");

    // There should be no custom depth that doesn't exist in the schema.
    // We verify by deleting all representations for this memory and checking.
    store
        .connection_mut()
        .execute(
            "DELETE FROM memory_representations WHERE memory_id = ?1 AND representation_type = 'one_liner'",
            rusqlite::params![memory_id.as_str()],
        )
        .unwrap();

    let result = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::OneLiner)
        .unwrap();
    assert!(result.is_none(), "expected None after deleting the row");
}

// ── has_active_embedding ──────────────────────────────────────────────────────

#[test]
fn has_active_embedding_false_before_record_embedding() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("embtest");
    store
        .ensure_project(&project, "EmbTest", None, None)
        .unwrap();
    let memory_id = seed_memory(&mut store, &project, "Embedding test body.");

    let repr_id = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary repr must exist");

    let exists = store
        .has_active_embedding(&repr_id, "fake", "test-small")
        .unwrap();
    assert!(!exists, "no embedding recorded yet — must be false");
}

#[test]
fn has_active_embedding_true_after_record_embedding() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("embtest2");
    store
        .ensure_project(&project, "EmbTest2", None, None)
        .unwrap();
    let memory_id = seed_memory(&mut store, &project, "Another embedding test.");

    let repr_id = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary repr must exist");

    let vector: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
    store
        .record_embedding(&NewEmbedding {
            memory_id: &memory_id,
            representation_id: &repr_id,
            representation_type: RepresentationDepth::Summary.as_str(),
            provider: "fake",
            model: "test-small",
            vector: &vector,
        })
        .unwrap();

    let exists = store
        .has_active_embedding(&repr_id, "fake", "test-small")
        .unwrap();
    assert!(exists, "embedding was recorded — must be true");
}

#[test]
fn has_active_embedding_false_after_mark_stale() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("embtest3");
    store
        .ensure_project(&project, "EmbTest3", None, None)
        .unwrap();
    let memory_id = seed_memory(&mut store, &project, "Mark stale test.");

    let repr_id = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary repr must exist");

    let vector: Vec<f32> = vec![0.5, 0.6, 0.7, 0.8];
    let embedding_id = store
        .record_embedding(&NewEmbedding {
            memory_id: &memory_id,
            representation_id: &repr_id,
            representation_type: RepresentationDepth::Summary.as_str(),
            provider: "fake",
            model: "test-small",
            vector: &vector,
        })
        .unwrap();

    // Mark it stale.
    store.mark_embedding_stale(&embedding_id).unwrap();

    let exists = store
        .has_active_embedding(&repr_id, "fake", "test-small")
        .unwrap();
    assert!(!exists, "embedding is stale — must return false");
}

// ── record_failed_embedding_job ───────────────────────────────────────────────

#[test]
fn record_failed_embedding_job_inserts_row_with_status_failed() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("failjob");
    store
        .ensure_project(&project, "FailJob", None, None)
        .unwrap();
    let memory_id = seed_memory(&mut store, &project, "Failed embedding test.");

    let repr_id = store
        .repr_id_for_depth(&memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary repr must exist");

    store
        .record_failed_embedding_job(
            &memory_id,
            &repr_id,
            RepresentationDepth::Summary,
            "fake",
            "test-small",
            "provider returned error: timeout",
        )
        .unwrap();

    // Read back via raw connection and verify the row.
    let (status, error): (String, String) = store
        .connection()
        .query_row(
            "SELECT status, error FROM embedding_jobs
             WHERE memory_id = ?1 AND representation_id = ?2",
            rusqlite::params![memory_id.as_str(), repr_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("failed job row must exist");

    assert_eq!(status, "failed");
    assert_eq!(error, "provider returned error: timeout");
}
