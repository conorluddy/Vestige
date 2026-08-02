//! Integration tests for the semantic dedup leg added to
//! `vestige_engine::candidate::propose_candidate` (issue #133, commit 3 of 4).
//!
//! Uses real SQLite in a `TempDir` and `FakeEmbeddingProvider` so no network
//! or model downloads are required. `FakeEmbeddingProvider` is SHA-256-tiled
//! and explicitly NOT semantically meaningful (see its doc comment) — it
//! cannot be used to prove "finds a real paraphrase". Instead these tests rig
//! the plumbing directly: they store a chosen vector as a memory's embedding,
//! independent of that memory's actual body text, then assert the dedup probe
//! follows the vector rather than the words. That proves the wiring (cosine
//! lookup -> merge -> `matched_via`) works; it does not validate real
//! semantic similarity, which needs `fastembed`/`ollama` and is out of scope
//! here (would break the no-network test invariant).

use tempfile::TempDir;
use vestige_core::{
    build_bundle, MemoryId, MemoryType, NewCandidate, NewMemory, ProjectId, RepresentationDepth,
};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::candidate::{propose_candidate, MatchedVia};
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

fn seed_memory(
    store: &mut Store,
    project_id: &ProjectId,
    body: &str,
    memory_type: MemoryType,
) -> MemoryId {
    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: memory_type,
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

/// Store `provider.embed(text)` as `memory_id`'s embedding, independent of
/// the memory's actual stored body. This is the trick that lets a test rig a
/// vector-space match without a real semantically-meaningful provider.
///
/// Copied from `crates/vestige-engine/tests/search.rs` (`embed_memory`) —
/// duplicated rather than shared because that helper is private to its own
/// integration-test binary.
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

fn new_candidate(project_id: ProjectId, body: &str, memory_type: MemoryType) -> NewCandidate {
    NewCandidate {
        project_id,
        proposed_type: memory_type,
        body: body.to_string(),
        rationale: Some("test rationale".to_string()),
        title_override: None,
        importance: 0.6,
        confidence: 0.8,
        source: None,
        duplicate_of_memory_id: None,
        duplicate_of_candidate_id: None,
    }
}

// === TESTS ===

/// Rigged-plumbing test (see module doc): a memory's stored body ("A") shares
/// no FTS tokens with the candidate's probe text ("B"), so the lexical leg
/// legitimately misses it. The memory's embedding is rigged to be
/// `provider.embed(B)` — the exact vector the semantic leg will look up —
/// so only the semantic leg can find it. Asserts `matched_via == Semantic`.
#[test]
fn propose_finds_semantically_rigged_match_lexically_disjoint() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("semantic-rigged");
    seed_project(&mut store, &project);
    let provider = FakeEmbeddingProvider::default();

    let memory_body = "Zebras migrate across the Serengeti every dry season.";
    let probe_text = "Quarterly revenue exceeded analyst forecasts by a wide margin.";

    let memory_id = seed_memory(&mut store, &project, memory_body, MemoryType::Note);
    // Rig the embedding to match the probe text, not the memory's own body —
    // this is the "plumbing", not real semantic detection.
    embed_memory(&mut store, &memory_id, &provider, probe_text);

    let outcome = propose_candidate(
        &mut store,
        &project,
        new_candidate(project.clone(), probe_text, MemoryType::Note),
        Some(&provider),
    )
    .unwrap();

    assert!(
        !outcome.similar_memories.is_empty(),
        "semantic leg should surface the rigged memory"
    );
    let hit = outcome
        .similar_memories
        .iter()
        .find(|m| m.id == memory_id)
        .expect("rigged memory must be present in similar_memories");
    assert_eq!(
        hit.matched_via,
        MatchedVia::Semantic,
        "lexically-disjoint bodies must not trip the lexical leg"
    );
}

/// A memory found by BOTH legs (lexical token overlap AND a rigged matching
/// embedding) must be merged into a single `SimilarMemory` with
/// `matched_via == Both`, not appear twice.
#[test]
fn propose_merges_both_leg_match_into_single_both_hit() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("semantic-both");
    seed_project(&mut store, &project);
    let provider = FakeEmbeddingProvider::default();

    // Shares the token "SQLite" / "storage" with the candidate body below, so
    // the lexical leg will find it.
    let memory_body = "SQLite is chosen as the canonical storage engine for Vestige.";
    let probe_text = "SQLite canonical storage engine selected for reliability.";

    let memory_id = seed_memory(&mut store, &project, memory_body, MemoryType::Decision);
    // Rig the embedding to also match the probe text, so the semantic leg
    // finds the same memory.
    embed_memory(&mut store, &memory_id, &provider, probe_text);

    let outcome = propose_candidate(
        &mut store,
        &project,
        new_candidate(project.clone(), probe_text, MemoryType::Decision),
        Some(&provider),
    )
    .unwrap();

    let matches: Vec<_> = outcome
        .similar_memories
        .iter()
        .filter(|m| m.id == memory_id)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "a memory found by both legs must be merged into exactly one hit"
    );
    assert_eq!(matches[0].matched_via, MatchedVia::Both);
}

/// PRD §13 invariant: dedup failure (or unavailability) must never block
/// proposal. Covers both `embedding_provider: None` (pre-commit-4 call sites)
/// and `Some(provider)` against a project with zero embeddings (cold start) —
/// both must behave identically to lexical-only dedup, no error, no panic.
#[test]
fn propose_with_no_embeddings_behaves_identically_regardless_of_provider() {
    let (_tmp, mut store_a) = open_store();
    let project_a = ProjectId::from_slug("cold-start-none");
    seed_project(&mut store_a, &project_a);
    seed_memory(
        &mut store_a,
        &project_a,
        "SQLite is chosen as the canonical storage engine for Vestige.",
        MemoryType::Decision,
    );

    let outcome_none = propose_candidate(
        &mut store_a,
        &project_a,
        new_candidate(
            project_a.clone(),
            "SQLite canonical storage engine selected for reliability.",
            MemoryType::Decision,
        ),
        None,
    )
    .unwrap();

    let (_tmp2, mut store_b) = open_store();
    let project_b = ProjectId::from_slug("cold-start-some");
    seed_project(&mut store_b, &project_b);
    seed_memory(
        &mut store_b,
        &project_b,
        "SQLite is chosen as the canonical storage engine for Vestige.",
        MemoryType::Decision,
    );
    let provider = FakeEmbeddingProvider::default();
    // No `embed_memory` call here — this project has zero embeddings, so the
    // semantic leg must cold-start to empty rather than error.

    let outcome_some = propose_candidate(
        &mut store_b,
        &project_b,
        new_candidate(
            project_b.clone(),
            "SQLite canonical storage engine selected for reliability.",
            MemoryType::Decision,
        ),
        Some(&provider),
    )
    .unwrap();

    // Both must find the lexically-similar memory via the lexical leg alone,
    // and neither must have a `Semantic`/`Both` hit (no embeddings exist).
    assert_eq!(
        outcome_none.similar_memories.len(),
        outcome_some.similar_memories.len(),
        "None vs. Some(provider)-with-no-embeddings must produce identical result counts"
    );
    assert!(!outcome_none.similar_memories.is_empty());
    assert!(outcome_some
        .similar_memories
        .iter()
        .all(|m| m.matched_via == MatchedVia::Lexical));
}
