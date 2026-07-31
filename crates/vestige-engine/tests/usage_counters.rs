//! Integration tests for engine-level usage-counter wiring (#128 T4).
//!
//! `crates/vestige-store/tests/usage_counters.rs` proves `bump_recall_stats`
//! / `bump_expand_stats` behave correctly in isolation. This file proves the
//! engine call sites actually invoke them — every `search_*` branch and
//! `expand_memory` — and that a bump failure never turns a successful recall
//! into an error (PRD §10.5 posture, extended to usage counters).

use tempfile::TempDir;
use vestige_config::TracesConfig;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::context::expand_memory;
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
        .ensure_project(project_id, "Usage Counters Test Project", None, None)
        .unwrap();
}

fn record_memory(store: &mut Store, project_id: &ProjectId, body: &str) -> MemoryId {
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
    memory_id: &MemoryId,
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

fn recall_count(store: &Store, id: &MemoryId) -> i64 {
    store.get_memory(id).unwrap().unwrap().memory.recall_count
}

fn expand_count(store: &Store, id: &MemoryId) -> i64 {
    store.get_memory(id).unwrap().unwrap().memory.expand_count
}

// === SEARCH BRANCHES ===

#[test]
fn search_lexical_bumps_recall_count_for_every_result() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-lexical");
    seed_project(&mut store, &project);
    let id = record_memory(
        &mut store,
        &project,
        "Lexical recall bumps the usage counter.",
    );

    assert_eq!(
        recall_count(&store, &id),
        0,
        "unrecalled memory starts at 0"
    );

    let outcome = search_lexical(
        &store,
        &project,
        "usage counter",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert!(outcome.scored.iter().any(|c| c.card.id == id));
    assert_eq!(
        recall_count(&store, &id),
        1,
        "lexical search must bump recall_count"
    );
}

#[test]
fn search_semantic_bumps_recall_count_for_every_result() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-semantic");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Semantic recall bumps the usage counter.";
    let id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &id, &provider, text);

    assert_eq!(recall_count(&store, &id), 0);

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

    assert!(outcome.scored.iter().any(|c| c.card.id == id));
    assert_eq!(
        recall_count(&store, &id),
        1,
        "semantic search must bump recall_count"
    );
}

#[test]
fn search_hybrid_merged_bumps_recall_count_for_every_result() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-hybrid-merged");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Hybrid merged recall bumps the usage counter.";
    let id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &id, &provider, text);

    assert_eq!(recall_count(&store, &id), 0);

    let outcome = search_hybrid(
        &store,
        &project,
        "hybrid usage counter",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert_eq!(outcome.effective_mode, vestige_core::SearchMode::Hybrid);
    assert!(outcome.scored.iter().any(|c| c.card.id == id));
    assert_eq!(
        recall_count(&store, &id),
        1,
        "hybrid merged search must bump recall_count"
    );
}

#[test]
fn search_hybrid_fallback_bumps_recall_count_for_every_result() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-hybrid-fallback");
    seed_project(&mut store, &project);
    // No embeddings recorded — search_hybrid must fall back to lexical.
    let id = record_memory(
        &mut store,
        &project,
        "Hybrid fallback recall bumps the usage counter.",
    );

    assert_eq!(recall_count(&store, &id), 0);

    let provider = FakeEmbeddingProvider::default();
    let outcome = search_hybrid(
        &store,
        &project,
        "usage counter",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert_eq!(outcome.effective_mode, vestige_core::SearchMode::Lexical);
    assert!(outcome.scored.iter().any(|c| c.card.id == id));
    assert_eq!(
        recall_count(&store, &id),
        1,
        "hybrid-falling-back-to-lexical must still bump recall_count"
    );
}

// === EXPAND ===

#[test]
fn expand_memory_bumps_expand_count() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-expand");
    seed_project(&mut store, &project);
    let id = record_memory(&mut store, &project, "Expand bumps the usage counter.");

    assert_eq!(expand_count(&store, &id), 0);

    expand_memory(
        &store,
        &project,
        &id,
        RepresentationDepth::Summary,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    assert_eq!(
        expand_count(&store, &id),
        1,
        "expand must bump expand_count"
    );
    assert_eq!(
        recall_count(&store, &id),
        0,
        "expand must not touch recall_count"
    );
}

// === FAILURE ISOLATION ===

#[test]
fn search_still_returns_ok_when_recall_stats_bump_fails() {
    // `PRAGMA query_only` turns every write on this connection into an error
    // while leaving SELECTs intact — a lightweight way to force
    // `bump_recall_stats` to fail without touching engine internals. The
    // trace write fails the same way (already proven non-fatal elsewhere);
    // this test is specifically about the new counter bump.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-bump-failure");
    seed_project(&mut store, &project);
    let id = record_memory(&mut store, &project, "Bump failure must not fail search.");

    store
        .connection()
        .execute_batch("PRAGMA query_only = ON;")
        .unwrap();

    let outcome = search_lexical(
        &store,
        &project,
        "bump failure",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    );

    assert!(
        outcome.is_ok(),
        "a recall_count bump failure must not fail the search"
    );
    let outcome = outcome.unwrap();
    assert!(
        outcome.scored.iter().any(|c| c.card.id == id),
        "read path must still return results despite the write failure"
    );

    store
        .connection()
        .execute_batch("PRAGMA query_only = OFF;")
        .unwrap();
    assert_eq!(
        recall_count(&store, &id),
        0,
        "the bump genuinely failed (not a silent no-op) — recall_count stayed at 0"
    );
}

#[test]
fn expand_still_returns_ok_when_expand_stats_bump_fails() {
    // Mirror of the search-side test above, for the other half of the DoD:
    // `expand_memory` must survive a failed counter write the same way.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("usage-expand-failure");
    seed_project(&mut store, &project);
    let id = record_memory(
        &mut store,
        &project,
        "Expand bump failure must not fail expand.",
    );

    store
        .connection()
        .execute_batch("PRAGMA query_only = ON;")
        .unwrap();

    let outcome = expand_memory(
        &store,
        &project,
        &id,
        RepresentationDepth::Summary,
        Caller::Cli,
        &TracesConfig::default(),
    );

    assert!(
        outcome.is_ok(),
        "an expand_count bump failure must not fail the expand"
    );

    store
        .connection()
        .execute_batch("PRAGMA query_only = OFF;")
        .unwrap();
    assert_eq!(
        expand_count(&store, &id),
        0,
        "the bump genuinely failed (not a silent no-op) — expand_count stayed at 0"
    );
}
