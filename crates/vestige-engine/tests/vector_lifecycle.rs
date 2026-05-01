//! Integration tests for vector storage and nearest-neighbour retrieval (PR3).
//!
//! Invariants verified (from CLAUDE.md):
//!   1. record → query returns correct top hit (similarity ≈ 1.0).
//!   2. Soft-deleted memories are excluded from nearest_neighbours.
//!   3. Restore + re-embed brings memory back to results.
//!   4. Cross-project isolation: project A's query never returns project B's vector.
//!   5. Re-embedding same (repr_id, provider, model) does not duplicate rows.
//!   6. Dimension filter isolates embeddings from different models.
//!   7. embedding_status counts are correct.

use tempfile::TempDir;
use vestige_core::{build_bundle, EmbeddingId, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_embed::EmbeddingProvider;
use vestige_embed::FakeEmbeddingProvider;
use vestige_store::{NewEmbedding, Store, VectorFilter};

// === TEST HELPERS ===

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

/// Return the first representation_id for a memory (any type).
fn first_repr_id(store: &Store, memory_id: &MemoryId) -> String {
    store
        .connection()
        .query_row(
            "SELECT id FROM memory_representations WHERE memory_id = ?1 LIMIT 1",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

fn embed_memory(
    store: &mut Store,
    memory_id: &MemoryId,
    provider: &FakeEmbeddingProvider,
    text: &str,
) -> EmbeddingId {
    let repr_id = first_repr_id(store, memory_id);
    let vector = provider.embed(text).unwrap();
    let new = NewEmbedding {
        memory_id,
        representation_id: &repr_id,
        representation_type: "summary",
        provider: provider.provider_name(),
        model: provider.model_name(),
        vector: &vector,
    };
    store.record_embedding(&new).unwrap()
}

fn default_filter(provider: &FakeEmbeddingProvider) -> VectorFilter {
    VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: None,
    }
}

// === TEST 1: record → query returns top hit ===

#[test]
fn record_then_query_returns_it() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-record-query");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);
    let query_text = "Vestige is a local-first memory layer.";
    let memory_id = record_memory(&mut store, &project, query_text);
    embed_memory(&mut store, &memory_id, &provider, query_text);

    let query_vec = provider.embed(query_text).unwrap();
    let filter = default_filter(&provider);
    let hits = store
        .nearest_neighbours(&project, &query_vec, 5, &filter)
        .unwrap();

    assert_eq!(hits.len(), 1, "expected one hit");
    assert_eq!(hits[0].memory_id, memory_id);
    assert!(
        hits[0].similarity > 0.999,
        "similarity should be ~1.0 for the same vector, got {}",
        hits[0].similarity
    );
}

// === TEST 2: soft-deleted memory excluded ===

#[test]
fn soft_deleted_memory_excluded_from_nearest_neighbours() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-soft-delete");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Soft delete should remove from results.";
    let memory_id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &memory_id, &provider, text);

    // Soft-delete cascades status to 'stale' via trigger (PR1).
    store.forget_memory(&memory_id).unwrap();

    let query_vec = provider.embed(text).unwrap();
    let filter = default_filter(&provider);
    let hits = store
        .nearest_neighbours(&project, &query_vec, 5, &filter)
        .unwrap();

    assert!(
        hits.is_empty(),
        "soft-deleted memory must not appear in nearest_neighbours"
    );
}

// === TEST 3: restore + re-embed returns to results ===

#[test]
fn restore_then_reembed_returns_to_results() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-restore-reembed");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Memory that will be forgotten and restored.";
    let memory_id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &memory_id, &provider, text);

    // Forget → restore.
    store.forget_memory(&memory_id).unwrap();
    store.restore_memory(&memory_id).unwrap();

    // After restore, the embedding is stale (trigger cascaded on forget, not
    // on restore). Re-embed via INSERT OR REPLACE.
    embed_memory(&mut store, &memory_id, &provider, text);

    let query_vec = provider.embed(text).unwrap();
    let filter = default_filter(&provider);
    let hits = store
        .nearest_neighbours(&project, &query_vec, 5, &filter)
        .unwrap();

    assert_eq!(hits.len(), 1, "restored + re-embedded memory must reappear");
    assert_eq!(hits[0].memory_id, memory_id);
}

// === TEST 4: cross-project isolation ===

#[test]
fn cross_project_isolation() {
    // Single store, two projects. Each has one memory+embedding. Query in
    // project A must not return project B's vector. This proves the
    // `JOIN memories WHERE project_id = ?` guard.
    let (_tmp, mut store) = open_store();

    let project_a = ProjectId::from_slug("iso-project-a");
    let project_b = ProjectId::from_slug("iso-project-b");
    store.ensure_project(&project_a, "A", None, None).unwrap();
    store.ensure_project(&project_b, "B", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);

    let text_a = "Project A's unique memory content.";
    let text_b = "Project B's unique memory content.";

    let mem_a = record_memory(&mut store, &project_a, text_a);
    let mem_b = record_memory(&mut store, &project_b, text_b);
    embed_memory(&mut store, &mem_a, &provider, text_a);
    embed_memory(&mut store, &mem_b, &provider, text_b);

    let filter = default_filter(&provider);

    // Query project A with project B's vector — should still only return A.
    let query_b = provider.embed(text_b).unwrap();
    let hits_a = store
        .nearest_neighbours(&project_a, &query_b, 10, &filter)
        .unwrap();
    assert!(
        hits_a.iter().all(|h| h.memory_id == mem_a),
        "project A query must not return project B's memory"
    );

    // Query project B — must not return A's memory.
    let query_a = provider.embed(text_a).unwrap();
    let hits_b = store
        .nearest_neighbours(&project_b, &query_a, 10, &filter)
        .unwrap();
    assert!(
        hits_b.iter().all(|h| h.memory_id == mem_b),
        "project B query must not return project A's memory"
    );
}

// === TEST 5: re-embedding does not duplicate rows ===

#[test]
fn replace_existing_embedding_does_not_duplicate() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-no-dup");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);
    let memory_id = record_memory(&mut store, &project, "Deduplicated embedding.");
    let repr_id = first_repr_id(&store, &memory_id);

    let vec1 = provider.embed("first vector text").unwrap();
    let vec2 = provider.embed("second vector text").unwrap();

    // Embed once, then again with the same (repr_id, provider, model) but different text.
    store
        .record_embedding(&NewEmbedding {
            memory_id: &memory_id,
            representation_id: &repr_id,
            representation_type: "summary",
            provider: provider.provider_name(),
            model: provider.model_name(),
            vector: &vec1,
        })
        .unwrap();

    store
        .record_embedding(&NewEmbedding {
            memory_id: &memory_id,
            representation_id: &repr_id,
            representation_type: "summary",
            provider: provider.provider_name(),
            model: provider.model_name(),
            vector: &vec2,
        })
        .unwrap();

    // Exactly one embedding row, one vector row.
    let emb_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = ?1",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let vec_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_vectors WHERE embedding_id IN
             (SELECT id FROM memory_embeddings WHERE memory_id = ?1)",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        emb_count, 1,
        "must have exactly one embedding row after replace"
    );
    assert_eq!(
        vec_count, 1,
        "must have exactly one vector row after replace"
    );

    // The stored vector should be the second one (the replacement).
    let query_vec2 = provider.embed("second vector text").unwrap();
    let filter = default_filter(&provider);
    let hits = store
        .nearest_neighbours(&project, &query_vec2, 5, &filter)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].similarity > 0.999,
        "stored vector should be the replacement"
    );
}

// === TEST 6: dimensions filter isolates models ===

#[test]
fn dimensions_filter_isolates_models() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-dims");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider_64 = FakeEmbeddingProvider::new(64);
    let provider_128 = FakeEmbeddingProvider::new(128);

    let mem_64 = record_memory(&mut store, &project, "64-dim memory.");
    let mem_128 = record_memory(&mut store, &project, "128-dim memory.");

    let repr_64 = first_repr_id(&store, &mem_64);
    let repr_128 = first_repr_id(&store, &mem_128);

    let vec_64 = provider_64.embed("64-dim text").unwrap();
    let vec_128 = provider_128.embed("128-dim text").unwrap();

    store
        .record_embedding(&NewEmbedding {
            memory_id: &mem_64,
            representation_id: &repr_64,
            representation_type: "summary",
            provider: provider_64.provider_name(),
            // Use distinct model names so the unique index allows both rows
            model: "deterministic-sha256-64",
            vector: &vec_64,
        })
        .unwrap();

    store
        .record_embedding(&NewEmbedding {
            memory_id: &mem_128,
            representation_id: &repr_128,
            representation_type: "summary",
            provider: provider_128.provider_name(),
            model: "deterministic-sha256-128",
            vector: &vec_128,
        })
        .unwrap();

    // Query with 64-dim filter — only mem_64 should appear.
    let filter_64 = VectorFilter {
        provider: provider_64.provider_name().to_string(),
        model: "deterministic-sha256-64".to_string(),
        dimensions: 64,
        memory_type: None,
    };
    let hits_64 = store
        .nearest_neighbours(&project, &vec_64, 10, &filter_64)
        .unwrap();
    assert_eq!(hits_64.len(), 1, "only the 64-dim memory should appear");
    assert_eq!(hits_64[0].memory_id, mem_64);

    // Query with 128-dim filter — only mem_128 should appear.
    let filter_128 = VectorFilter {
        provider: provider_128.provider_name().to_string(),
        model: "deterministic-sha256-128".to_string(),
        dimensions: 128,
        memory_type: None,
    };
    let hits_128 = store
        .nearest_neighbours(&project, &vec_128, 10, &filter_128)
        .unwrap();
    assert_eq!(hits_128.len(), 1, "only the 128-dim memory should appear");
    assert_eq!(hits_128[0].memory_id, mem_128);
}

// === TEST 7: embedding_status counts correctly ===

#[test]
fn embedding_status_counts_correctly() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("test-status");
    store.ensure_project(&project, "Test", None, None).unwrap();

    let provider = FakeEmbeddingProvider::new(64);

    // Record 3 memories.
    let m1 = record_memory(&mut store, &project, "Memory one.");
    let m2 = record_memory(&mut store, &project, "Memory two.");
    let m3 = record_memory(&mut store, &project, "Memory three.");

    // Embed 2 of them (m1 and m2).
    embed_memory(&mut store, &m1, &provider, "Memory one.");
    let emb2_id = embed_memory(&mut store, &m2, &provider, "Memory two.");

    // Mark m2's embedding stale.
    store.mark_embedding_stale(&emb2_id).unwrap();

    // m3 is not embedded at all.
    let _ = m3;

    let status = store.embedding_status(&project).unwrap();

    assert_eq!(status.total_active_memories, 3, "3 active memories");
    // 3 memories × 2 embeddable reps each (summary + compressed) = 6.
    // RepresentationDepth::Compressed serialises as "compressed", not "compressed_body".
    assert_eq!(
        status.embeddable_representations, 6,
        "6 embeddable representations (2 per memory)"
    );
    // m1 has 1 active embedding (embedded 1 rep out of 2 embeddable)
    assert_eq!(
        status.embedded_representations, 1,
        "1 representation has an active embedding"
    );
    // m2's embedding was marked stale
    assert_eq!(status.stale_embeddings, 1, "1 stale embedding");
    // missing = 6 (embeddable) - 1 (embedded) - 1 (stale) = 4
    assert_eq!(status.missing_embeddings, 4, "4 missing embeddings");
    assert_eq!(status.failed_jobs, 0, "no failed jobs");

    // Provider/model are present since we have active embeddings.
    assert!(status.provider.is_some());
    assert!(status.model.is_some());
}

// === BONUS: EmbeddingId prefix validation ===

#[test]
fn embedding_id_has_correct_prefix() {
    let id = EmbeddingId::new();
    assert!(
        id.as_str().starts_with("emb_"),
        "EmbeddingId must start with 'emb_', got: {}",
        id.as_str()
    );
}
