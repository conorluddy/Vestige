//! Integration tests for `vestige-engine::replay`.
//!
//! Covers the PRD §15 M5 acceptance criteria and issue #59 DoD:
//!
//! - Identical corpus → empty diff (added=[], removed=[], score_changes=[]).
//! - New memory added since original → appears in `added`.
//! - Originally-returned memory forgotten → appears in `removed`.
//! - Original trace never mutated after replay.
//! - Replay writes a NEW `query_events` row with `params_json.replay_of`.
//! - Provider mismatch: original semantic, replay with no provider → `provider_match=false`.
//! - Wrong-prefix trace_id → clean error.
//! - Trace not in current project → `EngineError::TraceNotFound`.
//! - `ReplayResult` JSON shape deserializes cleanly.

use std::str::FromStr;

use tempfile::TempDir;

use vestige_config::TracesConfig;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::{replay::replay_trace, search::search_lexical, trace::Caller};
use vestige_store::{NewEmbedding, Store};

// ─────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_project(store: &mut Store, project_id: &ProjectId) {
    store
        .ensure_project(project_id, "Replay Test Project", None, None)
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

/// Fetch the last trace ID written for `project_id`.
fn last_trace_id(store: &Store, project_id: &ProjectId) -> String {
    store
        .fetch_last_trace_id(project_id.as_str())
        .unwrap()
        .expect("at least one trace must exist")
}

/// Read the raw `params_json` for a trace by ID.
fn params_json_for(store: &Store, trace_id: &str) -> Option<String> {
    store
        .connection()
        .query_row(
            "SELECT params_json FROM query_events WHERE id = ?1",
            rusqlite::params![trace_id],
            |r| r.get(0),
        )
        .ok()
        .flatten()
}

/// Count query_events rows for a project.
fn trace_count(store: &Store, project_id: &ProjectId) -> usize {
    store.query_event_count(project_id.as_str()).unwrap()
}

// ─────────────────────────────────────────────────────────────────
// === TEST 1: Identical corpus → empty diff ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn identical_corpus_produces_empty_diff() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-identical");
    seed_project(&mut store, &project);
    record_memory(
        &mut store,
        &project,
        "Local-first memory system for agents.",
    );

    // Run a search to produce a trace.
    search_lexical(
        &store,
        &project,
        "local-first",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_trace_id_str = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_trace_id_str).unwrap();

    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    assert_eq!(
        result.diff.added,
        Vec::<String>::new(),
        "identical corpus → no added"
    );
    assert_eq!(
        result.diff.removed,
        Vec::<String>::new(),
        "identical corpus → no removed"
    );
    assert!(
        result.diff.score_changes.is_empty(),
        "identical corpus → no score changes"
    );
    assert!(
        result.provider_match,
        "lexical original → provider_match=true"
    );
    assert!(!result.mode_fallback, "lexical → no fallback");
}

// ─────────────────────────────────────────────────────────────────
// === TEST 2: New memory added → appears in `added` ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn new_memory_since_original_appears_in_added() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-added");
    seed_project(&mut store, &project);

    // Record a memory and search.
    record_memory(&mut store, &project, "Trace replay for agents.");
    search_lexical(
        &store,
        &project,
        "trace replay",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_trace_id_str = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_trace_id_str).unwrap();

    // Add a second memory AFTER the original search.
    let new_id = record_memory(
        &mut store,
        &project,
        "Trace replay integration test memory.",
    );

    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    assert!(
        result.diff.added.contains(&new_id.as_str().to_string()),
        "newly added memory must appear in diff.added; got {:?}",
        result.diff.added,
    );
}

// ─────────────────────────────────────────────────────────────────
// === TEST 3: Forgotten memory → appears in `removed` ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn forgotten_memory_appears_in_removed() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-removed");
    seed_project(&mut store, &project);

    // Record a memory and search so it appears in results.
    let id = record_memory(
        &mut store,
        &project,
        "Replay forget test memory for agents.",
    );
    search_lexical(
        &store,
        &project,
        "replay forget",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_trace_id_str = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_trace_id_str).unwrap();

    // Forget the memory AFTER the original search.
    store.forget_memory(&id).unwrap();

    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    assert!(
        result.diff.removed.contains(&id.as_str().to_string()),
        "forgotten memory must appear in diff.removed; got {:?}",
        result.diff.removed,
    );
}

// ─────────────────────────────────────────────────────────────────
// === TEST 4: Original trace never mutated ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn original_trace_is_never_mutated() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-immutable");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Original trace immutability test.");

    search_lexical(
        &store,
        &project,
        "immutability",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_id = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_id).unwrap();

    // Capture original row data before replay.
    let original_created_at: String = store
        .connection()
        .query_row(
            "SELECT created_at FROM query_events WHERE id = ?1",
            rusqlite::params![&original_id],
            |r| r.get(0),
        )
        .unwrap();

    let original_result_ids: Option<String> = store
        .connection()
        .query_row(
            "SELECT result_ids_json FROM query_events WHERE id = ?1",
            rusqlite::params![&original_id],
            |r| r.get(0),
        )
        .unwrap();

    let original_params: Option<String> = params_json_for(&store, &original_id);

    // Run replay.
    replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    // Assert original row unchanged.
    let after_created_at: String = store
        .connection()
        .query_row(
            "SELECT created_at FROM query_events WHERE id = ?1",
            rusqlite::params![&original_id],
            |r| r.get(0),
        )
        .unwrap();
    let after_result_ids: Option<String> = store
        .connection()
        .query_row(
            "SELECT result_ids_json FROM query_events WHERE id = ?1",
            rusqlite::params![&original_id],
            |r| r.get(0),
        )
        .unwrap();
    let after_params: Option<String> = params_json_for(&store, &original_id);

    assert_eq!(
        original_created_at, after_created_at,
        "original trace created_at must not change"
    );
    assert_eq!(
        original_result_ids, after_result_ids,
        "original trace result_ids must not change"
    );
    assert_eq!(
        original_params, after_params,
        "original trace params_json must not change"
    );
}

// ─────────────────────────────────────────────────────────────────
// === TEST 5: Replay writes NEW trace row with `replay_of` ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn replay_writes_new_trace_tagged_with_replay_of() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-new-trace");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Replay trace tagging test.");

    search_lexical(
        &store,
        &project,
        "trace tagging",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_id = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_id).unwrap();
    let before_count = trace_count(&store, &project);

    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    // One new row must have been written.
    let after_count = trace_count(&store, &project);
    assert_eq!(after_count, before_count + 1, "exactly one new trace row");

    // The new row must have a different ID.
    let replay_id = &result.replay_trace_id;
    assert_ne!(
        replay_id, &original_id,
        "replay must have a fresh trace_<ULID>"
    );
    assert!(
        replay_id.starts_with("trace_"),
        "replay trace id must start with trace_"
    );

    // The replay row's params_json must contain `replay_of = <original_id>`.
    let params_raw = params_json_for(&store, replay_id).expect("replay row must have params_json");
    let params: serde_json::Value = serde_json::from_str(&params_raw).unwrap();
    assert_eq!(
        params["replay_of"].as_str(),
        Some(original_id.as_str()),
        "params_json.replay_of must equal original trace id; got: {params_raw}"
    );

    // The original row must NOT have a `replay_of` field.
    let original_params_raw = params_json_for(&store, &original_id);
    if let Some(raw) = original_params_raw {
        let orig_params: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        assert!(
            orig_params["replay_of"].is_null(),
            "original trace must not have replay_of set"
        );
    }
}

// ─────────────────────────────────────────────────────────────────
// === TEST 6: Provider mismatch → provider_match=false ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn semantic_original_with_no_provider_gives_provider_mismatch() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-provider-mismatch");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Provider mismatch detection for replay.";
    let id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &id, &provider, text);

    // Run a semantic search to create the original trace.
    vestige_engine::search::search_semantic(
        &store,
        &project,
        "provider mismatch",
        None,
        10,
        &provider,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    let original_id = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_id).unwrap();

    // Replay with NO provider (simulates provider removed/unavailable).
    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    assert!(
        !result.provider_match,
        "semantic original with no provider must set provider_match=false"
    );
    assert!(result.mode_fallback, "must flag mode_fallback=true");
    // Replay must still produce results (lexical fallback).
    // The result set may be empty if FTS doesn't match, but no error.
}

// ─────────────────────────────────────────────────────────────────
// === TEST 7: Wrong-prefix trace_id → clean error ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn wrong_prefix_trace_id_returns_parse_error() {
    // `TraceId::from_str` enforces the `trace_` prefix — a `mem_` prefix
    // must fail before even reaching the engine.
    let result = vestige_core::TraceId::from_str("mem_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    assert!(result.is_err(), "mem_ prefix must be rejected");

    let result = vestige_core::TraceId::from_str("proj_something");
    assert!(result.is_err(), "proj_ prefix must be rejected");
}

// ─────────────────────────────────────────────────────────────────
// === TEST 8: Trace not in current project → TraceNotFound ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn trace_from_other_project_returns_trace_not_found() {
    let (_tmp, mut store) = open_store();
    let project_a = ProjectId::from_slug("replay-scope-a");
    let project_b = ProjectId::from_slug("replay-scope-b");
    seed_project(&mut store, &project_a);
    seed_project(&mut store, &project_b);

    record_memory(&mut store, &project_a, "Project A memory.");
    search_lexical(
        &store,
        &project_a,
        "project a",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let trace_id_str = last_trace_id(&store, &project_a);
    let trace_id = vestige_core::TraceId::from_str(&trace_id_str).unwrap();

    // Try to replay the project-A trace from project-B's scope.
    let err = replay_trace(&store, None, &project_b, &trace_id, Caller::Cli)
        .expect_err("cross-project replay must fail");

    assert!(
        matches!(
            err,
            vestige_engine::error::EngineError::TraceNotFound { .. }
        ),
        "expected TraceNotFound, got: {err}"
    );
}

// ─────────────────────────────────────────────────────────────────
// === TEST 9: JSON shape matches PRD §10.3 ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn replay_result_json_shape_matches_prd_10_3() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-json-shape");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "JSON shape validation memory.");

    search_lexical(
        &store,
        &project,
        "json shape",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let original_id = last_trace_id(&store, &project);
    let trace_id = vestige_core::TraceId::from_str(&original_id).unwrap();

    let result = replay_trace(&store, None, &project, &trace_id, Caller::Cli).unwrap();

    // Serialize and deserialize as Value for shape validation.
    let json_str = serde_json::to_string(&result).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Top-level required fields per PRD §10.3 replay output.
    assert!(json["trace_id"].is_string(), "trace_id must be string");
    assert!(json["original"].is_object(), "original must be object");
    assert!(json["current"].is_object(), "current must be object");
    assert!(json["diff"].is_object(), "diff must be object");
    assert!(
        json["provider_match"].is_boolean(),
        "provider_match must be boolean"
    );
    assert!(
        json["mode_fallback"].is_boolean(),
        "mode_fallback must be boolean"
    );
    assert!(
        json["replay_trace_id"].is_string(),
        "replay_trace_id must be string"
    );
    assert!(
        json["corpus_size"].is_number(),
        "corpus_size must be number"
    );

    // original / current sub-shapes.
    assert!(
        json["original"]["result_ids"].is_array(),
        "original.result_ids must be array"
    );
    assert!(
        json["original"]["scores"].is_array(),
        "original.scores must be array"
    );
    assert!(
        json["current"]["result_ids"].is_array(),
        "current.result_ids must be array"
    );
    assert!(
        json["current"]["scores"].is_array(),
        "current.scores must be array"
    );

    // diff sub-shape.
    assert!(json["diff"]["added"].is_array(), "diff.added must be array");
    assert!(
        json["diff"]["removed"].is_array(),
        "diff.removed must be array"
    );
    assert!(
        json["diff"]["score_changes"].is_array(),
        "diff.score_changes must be array"
    );
}

// ─────────────────────────────────────────────────────────────────
// === TEST 10: Replay-of-replay chain is allowed ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn replay_of_replay_is_allowed_and_produces_new_trace() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("replay-chain");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Replay chain test memory.");

    // Original search → trace 1.
    search_lexical(
        &store,
        &project,
        "replay chain",
        None,
        10,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();
    let trace1_id_str = last_trace_id(&store, &project);
    let trace1_id = vestige_core::TraceId::from_str(&trace1_id_str).unwrap();

    // Replay trace 1 → trace 2.
    let result1 = replay_trace(&store, None, &project, &trace1_id, Caller::Cli).unwrap();
    let trace2_id = vestige_core::TraceId::from_str(&result1.replay_trace_id).unwrap();

    // Replay trace 2 → trace 3.
    let result2 = replay_trace(&store, None, &project, &trace2_id, Caller::Cli).unwrap();

    assert_ne!(
        result2.replay_trace_id, result1.replay_trace_id,
        "replay of replay must produce a new trace id"
    );
    assert!(
        result2.replay_trace_id.starts_with("trace_"),
        "chained replay must still produce a trace_ id"
    );
}
