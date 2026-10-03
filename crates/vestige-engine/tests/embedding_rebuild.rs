//! Rebuild failure must preserve the working index, including SQL failures.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbedError, EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::embed::{embed_all, rebuild_embeddings};
use vestige_store::Store;

fn fixture() -> (TempDir, Store, ProjectId) {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    let project = ProjectId::from_slug("rebuild");
    store
        .ensure_project(&project, "Rebuild", None, None)
        .unwrap();
    store
        .record_memory(
            &build_bundle(
                &project,
                NewMemory {
                    r#type: MemoryType::Note,
                    body: "A memory for rebuilding.",
                    importance: 0.5,
                    source: None,
                },
            )
            .unwrap(),
        )
        .unwrap();
    embed_all(
        &mut store,
        &project,
        &FakeEmbeddingProvider::new(64),
        &depths(),
        false,
    )
    .unwrap();
    (tmp, store, project)
}

fn depths() -> [RepresentationDepth; 2] {
    [
        RepresentationDepth::Summary,
        RepresentationDepth::Compressed,
    ]
}

fn snapshot(store: &Store) -> Vec<(String, Vec<u8>)> {
    store
        .connection()
        .prepare("SELECT embedding_id, vector FROM memory_vectors ORDER BY embedding_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

struct BrokenProvider {
    invalid_vector: bool,
}

impl EmbeddingProvider for BrokenProvider {
    fn provider_name(&self) -> &'static str {
        "broken"
    }
    fn model_name(&self) -> &str {
        "broken"
    }
    fn dimensions(&self) -> usize {
        32
    }
    fn embed(&self, _: &str) -> Result<Vec<f32>, EmbedError> {
        if self.invalid_vector {
            Ok(vec![f32::NAN; 32])
        } else {
            Err(EmbedError::Backend("fixture failure".into()))
        }
    }
}

#[test]
fn inference_and_invalid_vectors_preserve_existing_index() {
    let (_tmp, mut store, project) = fixture();
    let before = snapshot(&store);
    for invalid_vector in [false, true] {
        assert!(rebuild_embeddings(
            &mut store,
            &project,
            &BrokenProvider { invalid_vector },
            &depths()
        )
        .is_err());
        assert_eq!(before, snapshot(&store));
    }
}

#[test]
fn insertion_failure_rolls_back_the_old_vectors_and_partial_replacement() {
    let (_tmp, mut store, project) = fixture();
    let before = snapshot(&store);
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER reject_rebuild BEFORE INSERT ON memory_embeddings
        WHEN NEW.dimensions = 32 AND NEW.representation_type = 'compressed'
        BEGIN SELECT RAISE(ABORT, 'fixture rejects second replacement'); END;",
        )
        .unwrap();
    assert!(rebuild_embeddings(
        &mut store,
        &project,
        &FakeEmbeddingProvider::new(32),
        &depths()
    )
    .is_err());
    assert_eq!(before, snapshot(&store));
}

#[test]
fn successful_rebuild_replaces_provider_dimensions_and_preserves_other_projects() {
    let (_tmp, mut store, project) = fixture();
    let other = ProjectId::from_slug("other");
    store.ensure_project(&other, "Other", None, None).unwrap();
    store
        .record_memory(
            &build_bundle(
                &other,
                NewMemory {
                    r#type: MemoryType::Note,
                    body: "Other project's memory.",
                    importance: 0.5,
                    source: None,
                },
            )
            .unwrap(),
        )
        .unwrap();
    embed_all(
        &mut store,
        &other,
        &FakeEmbeddingProvider::new(64),
        &depths(),
        false,
    )
    .unwrap();
    let before = snapshot(&store);
    let results = rebuild_embeddings(
        &mut store,
        &project,
        &FakeEmbeddingProvider::new(32),
        &depths(),
    )
    .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        store.embedding_status(&project).unwrap().dimensions,
        Some(32)
    );
    assert_eq!(store.embedding_status(&other).unwrap().dimensions, Some(64));
    let after = snapshot(&store);
    assert_eq!(after.len(), before.len());
    assert_eq!(before.iter().filter(|row| after.contains(row)).count(), 2);
}

#[test]
fn concurrent_revision_invalidates_prepared_vectors_without_clearing_index() {
    use vestige_store::{NewEmbedding, ReplacementEmbedding};
    let (_tmp, mut store, project) = fixture();
    let before = snapshot(&store);
    let fetched = store
        .list_memories(&project, &Default::default())
        .unwrap()
        .remove(0);
    let repr = fetched
        .representations
        .iter()
        .find(|r| r.depth == RepresentationDepth::Summary)
        .unwrap();
    let repr_id = store
        .repr_id_for_depth(&fetched.memory.id, repr.depth)
        .unwrap()
        .unwrap();
    store
        .revise_memory(&fetched.memory.id, "A concurrent revision.")
        .unwrap();
    let replacement = ReplacementEmbedding {
        embedding: NewEmbedding {
            memory_id: &fetched.memory.id,
            representation_id: &repr_id,
            representation_type: "summary",
            provider: "fake",
            model: "deterministic-sha256",
            vector: &[1.0; 32],
        },
        content_hash: &repr.content_hash,
    };
    assert!(store
        .replace_project_embeddings(&project, &[replacement])
        .is_err());
    assert_eq!(before, snapshot(&store));
}
