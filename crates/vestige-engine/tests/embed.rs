//! Integration tests for `vestige_engine::embed`.
//!
//! Uses real SQLite in a `TempDir` and `FakeEmbeddingProvider` so no network
//! or model downloads are required.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbedError, EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::embed::{embed_all, embed_memory_representations, EmbedOutcome};
use vestige_store::Store;

// === HELPERS ===

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_project(store: &mut Store, project_id: &ProjectId) {
    store
        .ensure_project(project_id, "Test Project", None, None)
        .unwrap();
}

fn record_memory_fetched(
    store: &mut Store,
    project_id: &ProjectId,
    body: &str,
) -> vestige_core::FetchedMemory {
    let bundle = build_bundle(
        project_id,
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
    store.get_memory(&id).unwrap().unwrap()
}

fn default_depths() -> Vec<RepresentationDepth> {
    vec![
        RepresentationDepth::Summary,
        RepresentationDepth::Compressed,
    ]
}

/// Count active embeddings for a specific memory by checking embedding_status.
///
/// Uses `store.embedding_status` (project-scoped) so we need a project_id.
/// For per-memory counts we check if the memory's repr has an active embedding
/// via `has_active_embedding` — but since that's private to the engine, we use
/// `embedding_status.embedded_representations` as a proxy when testing a single
/// memory per project.
fn count_active_embeddings(
    store: &Store,
    project_id: &ProjectId,
    _memory_id: &vestige_core::MemoryId,
) -> u64 {
    store
        .embedding_status(project_id)
        .unwrap()
        .embedded_representations
}

// === DRY-RUN TESTS ===

#[test]
fn embed_memory_representations_dry_run_reports_would_embed_no_writes() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-dryrun");
    seed_project(&mut store, &project);

    let fetched = record_memory_fetched(&mut store, &project, "Dry run test memory.");
    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    let results =
        embed_memory_representations(&mut store, &fetched, &provider, &depths, true).unwrap();

    // Every result with a representation should be WouldEmbed.
    for result in &results {
        if result.outcome != EmbedOutcome::NoRepr {
            assert_eq!(
                result.outcome,
                EmbedOutcome::WouldEmbed,
                "dry-run must report WouldEmbed for depth {:?}",
                result.representation_type
            );
        }
    }

    // No rows written.
    let written = count_active_embeddings(&store, &project, &fetched.memory.id);
    assert_eq!(written, 0, "dry-run must not write any embedding rows");
}

// === EMBED THEN UNCHANGED TESTS ===

#[test]
fn embed_memory_representations_writes_embeddings_returns_embedded() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-write");
    seed_project(&mut store, &project);

    let fetched = record_memory_fetched(&mut store, &project, "Memory to embed.");
    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    let results =
        embed_memory_representations(&mut store, &fetched, &provider, &depths, false).unwrap();

    let embedded_count = results
        .iter()
        .filter(|r| r.outcome == EmbedOutcome::Embedded)
        .count();
    assert!(
        embedded_count > 0,
        "at least one representation should be embedded"
    );
    assert!(
        results.iter().all(|r| r.outcome != EmbedOutcome::Failed),
        "no failures expected with FakeEmbeddingProvider"
    );

    // Rows exist in the DB.
    let db_count = count_active_embeddings(&store, &project, &fetched.memory.id);
    assert_eq!(
        db_count, embedded_count as u64,
        "DB embedding count must match reported embedded count"
    );
}

#[test]
fn embed_memory_representations_second_call_returns_unchanged() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-idempotent");
    seed_project(&mut store, &project);

    let fetched = record_memory_fetched(&mut store, &project, "Idempotent embedding test.");
    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    // First call — embeds.
    embed_memory_representations(&mut store, &fetched, &provider, &depths, false).unwrap();

    // Re-fetch the memory (embeddings now exist in DB) and call again.
    let results =
        embed_memory_representations(&mut store, &fetched, &provider, &depths, false).unwrap();

    for result in &results {
        if result.outcome != EmbedOutcome::NoRepr {
            assert_eq!(
                result.outcome,
                EmbedOutcome::Unchanged,
                "second call must return Unchanged for depth {:?}",
                result.representation_type
            );
        }
    }
}

// === EMBED ALL TESTS ===

#[test]
fn embed_all_iterates_active_memories_and_embeds_them() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-all");
    seed_project(&mut store, &project);

    let bodies = ["First memory.", "Second memory.", "Third memory."];
    let mut ids = Vec::new();
    for body in &bodies {
        let fetched = record_memory_fetched(&mut store, &project, body);
        ids.push(fetched.memory.id.clone());
    }

    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    let results = embed_all(&mut store, &project, &provider, &depths, false).unwrap();

    let embedded_count = results
        .iter()
        .filter(|r| r.outcome == EmbedOutcome::Embedded)
        .count();
    // 3 memories × 2 depths = 6 embeddings (if all reps exist).
    assert!(
        embedded_count >= 3,
        "at least one embedding per memory expected, got {}",
        embedded_count
    );

    // Verify the project has at least as many active embeddings as memories.
    let status = store.embedding_status(&project).unwrap();
    assert!(
        status.embedded_representations >= ids.len() as u64,
        "expected at least one active embedding per memory, got {} for {} memories",
        status.embedded_representations,
        ids.len()
    );
}

#[test]
fn embed_all_dry_run_reports_would_embed_no_db_writes() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-all-dry");
    seed_project(&mut store, &project);

    record_memory_fetched(&mut store, &project, "Dry-run all test.");
    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    let results = embed_all(&mut store, &project, &provider, &depths, true).unwrap();

    for result in &results {
        if result.outcome != EmbedOutcome::NoRepr {
            assert_eq!(
                result.outcome,
                EmbedOutcome::WouldEmbed,
                "dry-run embed_all must report WouldEmbed"
            );
        }
    }

    // No embeddings written.
    let status = store.embedding_status(&project).unwrap();
    assert_eq!(
        status.embedded_representations, 0,
        "dry-run embed_all must not write any embedding rows"
    );
}

#[test]
fn embed_all_skips_soft_deleted_memories() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-skip-deleted");
    seed_project(&mut store, &project);

    let fetched = record_memory_fetched(&mut store, &project, "Will be soft-deleted.");
    store.forget_memory(&fetched.memory.id).unwrap();

    let provider = FakeEmbeddingProvider::default();
    let depths = default_depths();

    let results = embed_all(&mut store, &project, &provider, &depths, false).unwrap();

    // No results for a deleted memory — embed_all only processes active memories.
    assert!(
        results.is_empty(),
        "embed_all must skip soft-deleted memories, got {} results",
        results.len()
    );
}

// === PROVIDER DIMENSION MISMATCH ===

/// Provider that declares one dimension count but returns vectors of a
/// different length — simulates a misconfigured provider (e.g. declared
/// 384 but a model upgrade quietly switched to 768) that would otherwise
/// silently insert un-queryable rows.
struct LyingProvider {
    declared_dims: usize,
    actual_len: usize,
}

impl EmbeddingProvider for LyingProvider {
    fn provider_name(&self) -> &'static str {
        "fake"
    }
    fn model_name(&self) -> &str {
        "lying-test"
    }
    fn dimensions(&self) -> usize {
        self.declared_dims
    }
    fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(vec![0.0; self.actual_len])
    }
}

#[test]
fn embed_rejects_provider_with_mismatched_vector_length() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("emb-dim-mismatch");
    seed_project(&mut store, &project);

    let fetched = record_memory_fetched(&mut store, &project, "Mismatch test memory.");
    let memory_id = fetched.memory.id.clone();

    let provider = LyingProvider {
        declared_dims: 384,
        actual_len: 5,
    };
    let depths = default_depths();

    let results =
        embed_memory_representations(&mut store, &fetched, &provider, &depths, false).unwrap();

    // Every requested depth should fail with the mismatch — the boundary check
    // catches the bad row before record_embedding sees it.
    assert_eq!(results.len(), depths.len());
    for r in &results {
        assert_eq!(
            r.outcome,
            EmbedOutcome::Failed,
            "expected Failed, got {:?}",
            r.outcome
        );
        let msg = r
            .error
            .as_ref()
            .expect("Failed result must carry an error message");
        assert!(
            msg.contains("dimension mismatch"),
            "error must mention 'dimension mismatch', got: {msg}"
        );
        assert!(
            msg.contains("384"),
            "error must name declared dim 384, got: {msg}"
        );
        assert!(
            msg.contains('5'),
            "error must name actual length 5, got: {msg}"
        );
    }

    // No vector rows persisted — the bogus 5-element vector never reached the store.
    assert_eq!(
        count_active_embeddings(&store, &project, &memory_id),
        0,
        "no embeddings should have been written for a mismatched provider"
    );

    // One failed-job row per attempted depth, all with the lying provider's name.
    let failed_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs
             WHERE status = 'failed' AND memory_id = ?1 AND model = ?2",
            rusqlite::params![memory_id.as_str(), "lying-test"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        failed_count,
        depths.len() as i64,
        "expected one failed job row per attempted depth"
    );
}
