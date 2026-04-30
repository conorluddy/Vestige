//! Integration tests for `vestige_engine::search`.
//!
//! Uses real SQLite in a `TempDir` and `FakeEmbeddingProvider` so no network
//! or model downloads are required.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId, SearchMode};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::search::{search_hybrid, search_lexical, search_semantic};
use vestige_store::{NewEmbedding, Store};

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

fn record_memory(store: &mut Store, project_id: &ProjectId, body: &str) -> vestige_core::MemoryId {
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
    id
}

fn embed_memory(
    store: &mut Store,
    memory_id: &vestige_core::MemoryId,
    provider: &FakeEmbeddingProvider,
    text: &str,
) {
    // Use the summary representation (first one).
    let repr_id: String = store
        .connection()
        .query_row(
            "SELECT id FROM memory_representations WHERE memory_id = ?1 AND representation_type = 'summary' LIMIT 1",
            rusqlite::params![memory_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    let vector = provider.embed(text).unwrap();
    store
        .record_embedding(&NewEmbedding {
            memory_id,
            representation_id: &repr_id,
            representation_type: "summary",
            provider: provider.provider_name(),
            model: provider.model_name(),
            vector: &vector,
        })
        .unwrap();
}

// === LEXICAL TESTS ===

#[test]
fn search_lexical_happy_path_returns_ranked_hits() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("lex-happy");
    seed_project(&mut store, &project);
    record_memory(
        &mut store,
        &project,
        "Vestige is a local-first memory layer for coding agents.",
    );

    let outcome = search_lexical(&store, &project, "memory layer", None, 10).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Lexical);
    assert!(outcome.warnings.is_empty());
    assert!(!outcome.scored.is_empty(), "expected at least one result");
}

#[test]
fn search_lexical_empty_query_returns_empty_no_warning() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("lex-empty");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Some content.");

    let outcome = search_lexical(&store, &project, "", None, 10).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Lexical);
    assert!(
        outcome.warnings.is_empty(),
        "empty query must not produce warnings"
    );
    assert!(
        outcome.scored.is_empty(),
        "empty query must return no results"
    );
}

#[test]
fn search_lexical_whitespace_only_query_returns_empty_no_warning() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("lex-whitespace");
    seed_project(&mut store, &project);

    let outcome = search_lexical(&store, &project, "   \t  ", None, 10).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Lexical);
    assert!(outcome.warnings.is_empty());
    assert!(outcome.scored.is_empty());
}

// === SEMANTIC TESTS ===

#[test]
fn search_semantic_no_embeddings_returns_empty_with_warning() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("sem-no-emb");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Memory with no embeddings.");

    let provider = FakeEmbeddingProvider::default();
    let outcome = search_semantic(&store, &project, "memory", None, 10, &provider).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Semantic);
    assert!(outcome.scored.is_empty(), "no embeddings → no results");
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("vestige embed --all"),
        "warning should hint at the fix, got: {}",
        outcome.warnings[0]
    );
}

#[test]
fn search_semantic_with_embeddings_returns_hits_scored_by_similarity() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("sem-with-emb");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let query = "local-first memory for agents";
    let memory_id = record_memory(&mut store, &project, query);
    embed_memory(&mut store, &memory_id, &provider, query);

    let outcome = search_semantic(&store, &project, query, None, 10, &provider).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Semantic);
    assert!(outcome.warnings.is_empty());
    assert!(!outcome.scored.is_empty(), "should have results");
    // The embedded query should score very close to 1.0 (same vector).
    let top = &outcome.scored[0];
    assert!(
        top.score > 0.99,
        "same-text embedding should score ~1.0, got {}",
        top.score
    );
}

// === HYBRID TESTS ===

#[test]
fn search_hybrid_no_embeddings_falls_back_to_lexical() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("hyb-no-emb");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Hybrid search without embeddings.");

    let provider = FakeEmbeddingProvider::default();
    let outcome = search_hybrid(&store, &project, "search", None, 10, &provider).unwrap();

    assert_eq!(
        outcome.effective_mode,
        SearchMode::Lexical,
        "must fall back to lexical"
    );
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("falling back to lexical"),
        "warning must describe the fallback, got: {}",
        outcome.warnings[0]
    );
}

#[test]
fn search_hybrid_with_both_legs_merges_and_deduplicates() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("hyb-both");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Coding agents benefit from structured memory.";
    let memory_id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &memory_id, &provider, text);

    let outcome = search_hybrid(&store, &project, "agents memory", None, 10, &provider).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Hybrid);
    assert!(outcome.warnings.is_empty());
    // The candidate from both legs should appear exactly once.
    let ids: Vec<_> = outcome.scored.iter().map(|s| &s.card.id).collect();
    let unique_count = {
        let mut deduped = ids.clone();
        deduped.dedup();
        deduped.len()
    };
    assert_eq!(
        ids.len(),
        unique_count,
        "hybrid results must not contain duplicate memory IDs"
    );
}

#[test]
fn search_hybrid_scores_are_in_expected_range() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("hyb-score-range");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Vestige stores coding knowledge persistently.";
    let memory_id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &memory_id, &provider, text);

    let outcome =
        search_hybrid(&store, &project, "vestige knowledge", None, 10, &provider).unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Hybrid);
    for result in &outcome.scored {
        assert!(
            result.score >= 0.0,
            "hybrid score must be non-negative, got {}",
            result.score
        );
    }
}
