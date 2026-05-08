//! Integration tests for `vestige_engine::search`.
//!
//! Uses real SQLite in a `TempDir` and `FakeEmbeddingProvider` so no network
//! or model downloads are required.

use tempfile::TempDir;
use vestige_config::TracesConfig;
use vestige_core::RepresentationDepth;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId, SearchMode};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::search::{search_hybrid, search_lexical, search_semantic};
use vestige_engine::Caller;
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
    let repr_id = store
        .repr_id_for_depth(memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary representation must exist");

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

    let outcome = search_lexical(
        &store,
        &project,
        "memory layer",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

    let outcome = search_lexical(
        &store,
        &project,
        "",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

    let outcome = search_lexical(
        &store,
        &project,
        "   \t  ",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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
    let outcome = search_semantic(
        &store,
        &project,
        "memory",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

    let outcome = search_semantic(
        &store,
        &project,
        query,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

#[test]
fn search_semantic_populates_score_parts_with_vector_component() {
    // PRD §11.3 / §19.4 — the JSON envelope must surface per-component scores
    // for every non-lexical search mode. Today's semantic path returns the
    // cosine similarity as `score`; the diagnostic must mirror that with
    // `vector == total == score` and the other components zero.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("sem-score-parts");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Score diagnostics must accompany semantic results.";
    let memory_id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &memory_id, &provider, text);

    let outcome = search_semantic(
        &store,
        &project,
        text,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert!(!outcome.scored.is_empty(), "should have results");
    let top = &outcome.scored[0];
    let parts = top
        .score_parts
        .expect("semantic results must carry score_parts diagnostic");
    assert_eq!(parts.fts, 0.0, "no FTS contribution on semantic-only path");
    assert_eq!(parts.importance, 0.0);
    assert_eq!(parts.type_boost, 0.0);
    assert!(
        parts.vector > 0.99,
        "vector component should equal cosine similarity, got {}",
        parts.vector
    );
    assert_eq!(
        parts.total, parts.vector,
        "semantic-only total must equal the vector component"
    );
    assert_eq!(
        top.score, parts.total,
        "displayed score and total must agree"
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
    let outcome = search_hybrid(
        &store,
        &project,
        "search",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

    let outcome = search_hybrid(
        &store,
        &project,
        "agents memory",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

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

    let outcome = search_hybrid(
        &store,
        &project,
        "vestige knowledge",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert_eq!(outcome.effective_mode, SearchMode::Hybrid);
    for result in &outcome.scored {
        assert!(
            result.score >= 0.0,
            "hybrid score must be non-negative, got {}",
            result.score
        );
    }
}

// === FORGET × SEMANTIC/HYBRID INVARIANTS ===
//
// nearest_neighbours filters m.status='active' AND e.status='active' at the
// store layer (crates/vestige-store/src/embeddings/nearest.rs:28-37) and that
// path has direct test coverage. These tests prove the same invariant holds
// at the engine entrypoints — the surface MCP and the CLI actually call. If a
// future refactor moves filtering up the stack, these regressions catch it.

#[test]
fn forget_excludes_from_search_semantic() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("forget-semantic");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let kept_text = "Surviving memory about hybrid search ranking.";
    let kept_id = record_memory(&mut store, &project, kept_text);
    embed_memory(&mut store, &kept_id, &provider, kept_text);

    let forgotten_text = "Forgotten memory about hybrid search ranking.";
    let forgotten_id = record_memory(&mut store, &project, forgotten_text);
    embed_memory(&mut store, &forgotten_id, &provider, forgotten_text);

    store.forget_memory(&forgotten_id).unwrap();

    let outcome = search_semantic(
        &store,
        &project,
        "hybrid search ranking",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    let returned_ids: Vec<_> = outcome.scored.iter().map(|s| &s.card.id).collect();
    assert!(
        !returned_ids.contains(&&forgotten_id),
        "forgotten memory must not appear in semantic results, got: {returned_ids:?}"
    );
    assert!(
        returned_ids.contains(&&kept_id),
        "non-forgotten memory must still appear, got: {returned_ids:?}"
    );
}

#[test]
fn forget_excludes_from_search_hybrid() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("forget-hybrid");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let kept_text = "Surviving memory about agent context windows.";
    let kept_id = record_memory(&mut store, &project, kept_text);
    embed_memory(&mut store, &kept_id, &provider, kept_text);

    let forgotten_text = "Forgotten memory about agent context windows.";
    let forgotten_id = record_memory(&mut store, &project, forgotten_text);
    embed_memory(&mut store, &forgotten_id, &provider, forgotten_text);

    store.forget_memory(&forgotten_id).unwrap();

    let outcome = search_hybrid(
        &store,
        &project,
        "agent context",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    // Hybrid merges both legs — the forgotten memory must drop from BOTH
    // (FTS via memory_after_soft_delete trigger, vectors via the e.status
    // filter). Either leg leaking it would break this assertion.
    let returned_ids: Vec<_> = outcome.scored.iter().map(|s| &s.card.id).collect();
    assert!(
        !returned_ids.contains(&&forgotten_id),
        "forgotten memory must not appear in hybrid results, got: {returned_ids:?}"
    );
    assert!(
        returned_ids.contains(&&kept_id),
        "non-forgotten memory must still appear, got: {returned_ids:?}"
    );
}

#[test]
fn restore_does_not_re_include_in_semantic_until_reindex() {
    // PRD §8.4: "embeddings are left stale after restore — they re-embed on
    // the next vestige embed run." Documents the deliberate choice that
    // restore alone does not bring a memory back to semantic recall; an
    // explicit re-embed is required.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("restore-semantic");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);

    // Two memories so search_semantic does not hit the cold-start path while
    // the test memory is in flight; the second memory keeps the project's
    // embedded_representations count above zero throughout.
    let anchor_text = "Anchor memory keeping the project embedded.";
    let anchor_id = record_memory(&mut store, &project, anchor_text);
    embed_memory(&mut store, &anchor_id, &provider, anchor_text);

    let target_text = "Target memory subjected to forget and restore.";
    let target_id = record_memory(&mut store, &project, target_text);
    embed_memory(&mut store, &target_id, &provider, target_text);

    // Sanity: present after embed.
    let pre = search_semantic(
        &store,
        &project,
        target_text,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    assert!(
        pre.scored.iter().any(|s| s.card.id == target_id),
        "target should be present before forget"
    );

    // After forget: target absent (the existing first invariant).
    store.forget_memory(&target_id).unwrap();
    let after_forget = search_semantic(
        &store,
        &project,
        target_text,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    assert!(
        !after_forget.scored.iter().any(|s| s.card.id == target_id),
        "target must be absent after forget"
    );

    // After restore but before re-embed: target STILL absent — embeddings
    // are stale, nearest_neighbours filters them out.
    store.restore_memory(&target_id).unwrap();
    let after_restore = search_semantic(
        &store,
        &project,
        target_text,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    assert!(
        !after_restore.scored.iter().any(|s| s.card.id == target_id),
        "target must remain absent after restore until re-embedded (PRD §8.4)"
    );

    // After explicit re-embed: target present again.
    embed_memory(&mut store, &target_id, &provider, target_text);
    let after_reembed = search_semantic(
        &store,
        &project,
        target_text,
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    assert!(
        after_reembed.scored.iter().any(|s| s.card.id == target_id),
        "target must reappear once re-embedded"
    );
}
