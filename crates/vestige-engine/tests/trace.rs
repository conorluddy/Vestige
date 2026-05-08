//! Integration tests for the engine tracing hook (`query_events` writes).
//!
//! All tests use real SQLite in a `TempDir`. The `FakeEmbeddingProvider` is
//! used for semantic and hybrid paths so no network or model downloads are
//! needed.
//!
//! # What's covered
//!
//! - Every recall path (search_lexical / search_semantic / search_hybrid /
//!   expand_memory / get_project_context) writes exactly one `query_events` row.
//! - Caller value (`cli` / `mcp`) is recorded correctly.
//! - `provider` and `provider_model` are populated for semantic / hybrid; null
//!   for lexical and non-search.
//! - `kind` is `search` / `expand` / `context` as expected.
//! - Mutation paths (record_memory, forget, restore) write zero trace rows.
//! - FIFO eviction: after writing `cap + N` traces only `cap` remain, and the
//!   surviving rows are the newest ones.
//! - `latency_ms` is non-negative (we do not assert a specific value since
//!   wall-clock time is non-deterministic, but it must be present).

use std::time::Instant;

use tempfile::TempDir;

use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_engine::{
    context::{expand_memory, get_project_context},
    search::{search_hybrid, search_lexical, search_semantic},
    trace::{write_trace_with_cap, Caller, TraceKind, TracePayload},
};
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
        .ensure_project(project_id, "Trace Test Project", None, None)
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

/// Count `query_events` rows for `project_id` via the store helper.
fn trace_count(store: &Store, project_id: &ProjectId) -> usize {
    store.query_event_count(project_id.as_str()).unwrap()
}

// ─────────────────────────────────────────────────────────────────
// === SEARCH LEXICAL ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn search_lexical_writes_one_trace_row() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-lex-one");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Memory about local-first systems.");

    assert_eq!(trace_count(&store, &project), 0, "no traces before search");

    search_lexical(&store, &project, "local-first", None, 10, Caller::Cli).unwrap();

    assert_eq!(
        trace_count(&store, &project),
        1,
        "exactly one trace after search"
    );
}

#[test]
fn search_lexical_trace_has_correct_kind_and_caller() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-lex-kind");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Anything.");

    search_lexical(&store, &project, "anything", None, 10, Caller::Mcp).unwrap();

    let row = store
        .connection()
        .query_row(
            "SELECT kind, caller, provider, provider_model, mode_requested, mode_resolved
             FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .unwrap();

    let (kind, caller, provider, provider_model, mode_req, mode_res) = row;
    assert_eq!(kind, "search");
    assert_eq!(caller, "mcp");
    assert!(provider.is_none(), "lexical must not record provider");
    assert!(
        provider_model.is_none(),
        "lexical must not record provider_model"
    );
    assert_eq!(mode_req.as_deref(), Some("lexical"));
    assert_eq!(mode_res.as_deref(), Some("lexical"));
}

#[test]
fn search_lexical_records_query_text_and_result_count() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-lex-qtext");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Vestige memory system for agents.");

    search_lexical(&store, &project, "vestige memory", None, 10, Caller::Cli).unwrap();

    let row = store
        .connection()
        .query_row(
            "SELECT query_text, result_count FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap();

    let (query_text, result_count) = row;
    assert_eq!(query_text.as_deref(), Some("vestige memory"));
    assert!(result_count >= 0, "result_count must be non-negative");
}

// ─────────────────────────────────────────────────────────────────
// === SEARCH SEMANTIC ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn search_semantic_writes_one_trace_with_provider_info() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-sem-one");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Semantic search over embeddings.";
    let id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &id, &provider, text);

    assert_eq!(trace_count(&store, &project), 0);

    search_semantic(
        &store,
        &project,
        "embeddings",
        None,
        10,
        &provider,
        Caller::Cli,
    )
    .unwrap();

    assert_eq!(trace_count(&store, &project), 1);

    let row = store
        .connection()
        .query_row(
            "SELECT kind, caller, provider, provider_model, mode_requested, mode_resolved
             FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .unwrap();

    let (kind, caller, provider_col, model_col, mode_req, mode_res) = row;
    assert_eq!(kind, "search");
    assert_eq!(caller, "cli");
    assert!(provider_col.is_some(), "semantic must record provider");
    assert_eq!(provider_col.as_deref(), Some(provider.provider_name()));
    assert!(model_col.is_some(), "semantic must record model");
    assert_eq!(model_col.as_deref(), Some(provider.model_name()));
    assert_eq!(mode_req.as_deref(), Some("semantic"));
    assert_eq!(mode_res.as_deref(), Some("semantic"));
}

#[test]
fn search_semantic_no_embeddings_still_writes_trace() {
    // Cold-start path: no embeddings, returns warning. One trace row must still
    // be written so the recall attempt is auditable.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-sem-cold");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Some content.");

    let provider = FakeEmbeddingProvider::new(64);
    search_semantic(&store, &project, "query", None, 10, &provider, Caller::Cli).unwrap();

    assert_eq!(
        trace_count(&store, &project),
        1,
        "cold-start must still write a trace"
    );
}

// ─────────────────────────────────────────────────────────────────
// === SEARCH HYBRID ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn search_hybrid_writes_one_trace_with_provider_info() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-hyb-one");
    seed_project(&mut store, &project);

    let provider = FakeEmbeddingProvider::new(64);
    let text = "Hybrid recall merges lexical and semantic.";
    let id = record_memory(&mut store, &project, text);
    embed_memory(&mut store, &id, &provider, text);

    search_hybrid(
        &store,
        &project,
        "hybrid recall",
        None,
        10,
        &provider,
        Caller::Cli,
    )
    .unwrap();

    assert_eq!(trace_count(&store, &project), 1);

    let row = store
        .connection()
        .query_row(
            "SELECT kind, provider, provider_model, mode_requested, mode_resolved
             FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .unwrap();

    let (kind, provider_col, model_col, mode_req, mode_res) = row;
    assert_eq!(kind, "search");
    assert!(provider_col.is_some(), "hybrid must record provider");
    assert!(model_col.is_some(), "hybrid must record model");
    assert_eq!(mode_req.as_deref(), Some("hybrid"));
    assert_eq!(mode_res.as_deref(), Some("hybrid"));
}

#[test]
fn search_hybrid_fallback_writes_single_trace_with_correct_modes() {
    // When hybrid falls back to lexical (no embeddings), the trace must record
    // mode_requested = "hybrid" and mode_resolved = "lexical" — NOT two separate
    // trace rows. This verifies we execute the fallback inline rather than
    // delegating to search_lexical which would write its own trace.
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-hyb-fallback");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "No embeddings yet.");

    let provider = FakeEmbeddingProvider::new(64);
    search_hybrid(
        &store,
        &project,
        "no embeddings",
        None,
        10,
        &provider,
        Caller::Cli,
    )
    .unwrap();

    assert_eq!(
        trace_count(&store, &project),
        1,
        "fallback must write exactly one trace"
    );

    let row = store
        .connection()
        .query_row(
            "SELECT mode_requested, mode_resolved FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .unwrap();

    let (mode_req, mode_res) = row;
    assert_eq!(
        mode_req.as_deref(),
        Some("hybrid"),
        "requested must be hybrid"
    );
    assert_eq!(
        mode_res.as_deref(),
        Some("lexical"),
        "resolved must be lexical on fallback"
    );
}

// ─────────────────────────────────────────────────────────────────
// === EXPAND ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn expand_memory_writes_one_trace_with_kind_expand() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-expand-one");
    seed_project(&mut store, &project);
    let id = record_memory(&mut store, &project, "Expand this memory please.");

    assert_eq!(trace_count(&store, &project), 0);

    expand_memory(
        &store,
        &project,
        &id,
        RepresentationDepth::Summary,
        Caller::Mcp,
    )
    .unwrap();

    assert_eq!(trace_count(&store, &project), 1);

    let row = store
        .connection()
        .query_row(
            "SELECT kind, caller, provider, provider_model, mode_requested, mode_resolved
             FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .unwrap();

    let (kind, caller, provider_col, model_col, mode_req, mode_res) = row;
    assert_eq!(kind, "expand");
    assert_eq!(caller, "mcp");
    assert!(provider_col.is_none(), "expand must not record provider");
    assert!(model_col.is_none(), "expand must not record provider_model");
    assert!(mode_req.is_none(), "expand has no mode");
    assert!(mode_res.is_none(), "expand has no mode");
}

// ─────────────────────────────────────────────────────────────────
// === CONTEXT ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn get_project_context_writes_one_trace_with_kind_context() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-ctx-one");
    seed_project(&mut store, &project);

    assert_eq!(trace_count(&store, &project), 0);

    get_project_context(&store, &project, "Trace Test", 8, 1200, Caller::Cli).unwrap();

    assert_eq!(trace_count(&store, &project), 1);

    let row = store
        .connection()
        .query_row(
            "SELECT kind, caller, provider, mode_requested FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .unwrap();

    let (kind, caller, provider_col, mode_req) = row;
    assert_eq!(kind, "context");
    assert_eq!(caller, "cli");
    assert!(provider_col.is_none(), "context must not record provider");
    assert!(mode_req.is_none(), "context has no mode");
}

// ─────────────────────────────────────────────────────────────────
// === MUTATIONS WRITE ZERO TRACES ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn mutations_write_zero_query_events() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-mut-zero");
    seed_project(&mut store, &project);

    // record_memory — mutation
    let id = record_memory(&mut store, &project, "Some note.");
    assert_eq!(
        trace_count(&store, &project),
        0,
        "record_memory must not write a trace"
    );

    // forget_memory — mutation
    store.forget_memory(&id).unwrap();
    assert_eq!(
        trace_count(&store, &project),
        0,
        "forget_memory must not write a trace"
    );

    // restore_memory — mutation
    store.restore_memory(&id).unwrap();
    assert_eq!(
        trace_count(&store, &project),
        0,
        "restore_memory must not write a trace"
    );
}

// ─────────────────────────────────────────────────────────────────
// === FIFO EVICTION ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn fifo_eviction_keeps_newest_rows_up_to_cap() {
    // Use cap = 5 so the test runs in milliseconds instead of inserting 10 000 rows.
    // write_trace_with_cap exposes the cap parameter for exactly this use.
    const TEST_CAP: usize = 5;
    const TOTAL_WRITES: usize = TEST_CAP + 3; // 8 writes → 3 oldest deleted

    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-evict");
    seed_project(&mut store, &project);

    for i in 0..TOTAL_WRITES {
        let query = format!("query {i}");
        write_trace_with_cap(
            &store,
            &TracePayload {
                project_id: &project,
                kind: TraceKind::Search,
                mode_requested: Some(vestige_core::SearchMode::Lexical),
                mode_resolved: Some(vestige_core::SearchMode::Lexical),
                query_text: Some(&query),
                params_json: None,
                caller: Caller::Cli,
                provider: None,
                provider_model: None,
                result_ids: Some(&[]),
                result_scores: Some(&[]),
                latency: std::time::Duration::from_millis(1),
            },
            TEST_CAP,
        );
    }

    let remaining = trace_count(&store, &project);
    assert_eq!(
        remaining, TEST_CAP,
        "eviction must leave exactly {TEST_CAP} rows"
    );

    // Verify oldest were deleted: the surviving query_text values should be
    // the newest (last 5 writes: "query 3" through "query 7").
    let mut texts: Vec<String> = store
        .connection()
        .prepare(
            "SELECT query_text FROM query_events WHERE project_id = ?1 ORDER BY created_at ASC",
        )
        .unwrap()
        .query_map(rusqlite::params![project.as_str()], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    texts.sort();

    let expected: Vec<String> = (TOTAL_WRITES - TEST_CAP..TOTAL_WRITES)
        .map(|i| format!("query {i}"))
        .collect();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();

    assert_eq!(
        texts, expected_sorted,
        "surviving rows must be the newest {TEST_CAP}"
    );
}

#[test]
fn eviction_is_per_project_not_global() {
    // Project A fills to cap; project B should be unaffected.
    const TEST_CAP: usize = 3;

    let (_tmp, mut store) = open_store();
    let project_a = ProjectId::from_slug("evict-proj-a");
    let project_b = ProjectId::from_slug("evict-proj-b");
    seed_project(&mut store, &project_a);
    seed_project(&mut store, &project_b);

    // Write cap+2 traces for project A.
    for i in 0..(TEST_CAP + 2) {
        let query = format!("a-query {i}");
        write_trace_with_cap(
            &store,
            &TracePayload {
                project_id: &project_a,
                kind: TraceKind::Search,
                mode_requested: Some(vestige_core::SearchMode::Lexical),
                mode_resolved: Some(vestige_core::SearchMode::Lexical),
                query_text: Some(&query),
                params_json: None,
                caller: Caller::Cli,
                provider: None,
                provider_model: None,
                result_ids: Some(&[]),
                result_scores: Some(&[]),
                latency: std::time::Duration::from_millis(1),
            },
            TEST_CAP,
        );
    }

    // Write 2 traces for project B.
    for i in 0..2 {
        let query = format!("b-query {i}");
        write_trace_with_cap(
            &store,
            &TracePayload {
                project_id: &project_b,
                kind: TraceKind::Search,
                mode_requested: Some(vestige_core::SearchMode::Lexical),
                mode_resolved: Some(vestige_core::SearchMode::Lexical),
                query_text: Some(&query),
                params_json: None,
                caller: Caller::Cli,
                provider: None,
                provider_model: None,
                result_ids: Some(&[]),
                result_scores: Some(&[]),
                latency: std::time::Duration::from_millis(1),
            },
            TEST_CAP,
        );
    }

    assert_eq!(
        trace_count(&store, &project_a),
        TEST_CAP,
        "project A must be capped"
    );
    assert_eq!(
        trace_count(&store, &project_b),
        2,
        "project B must not be affected by project A's eviction"
    );
}

// ─────────────────────────────────────────────────────────────────
// === LATENCY ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn search_lexical_trace_records_latency_ms() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-latency");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Latency should be recorded.");

    let t0 = Instant::now();
    search_lexical(&store, &project, "latency", None, 10, Caller::Cli).unwrap();
    let elapsed_ms = t0.elapsed().as_millis() as i64;

    let latency_ms: i64 = store
        .connection()
        .query_row(
            "SELECT latency_ms FROM query_events WHERE project_id = ?1",
            rusqlite::params![project.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    // The recorded latency must be non-negative and no larger than the total
    // elapsed time observed from the test (with a 50 ms grace margin for
    // scheduling jitter).
    assert!(latency_ms >= 0, "latency_ms must be non-negative");
    assert!(
        latency_ms <= elapsed_ms + 50,
        "latency_ms {latency_ms} must not exceed total elapsed {elapsed_ms} + grace"
    );
}

// ─────────────────────────────────────────────────────────────────
// === CALLER VARIANTS ===
// ─────────────────────────────────────────────────────────────────

#[test]
fn caller_cli_and_mcp_are_stored_correctly() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("trace-caller");
    seed_project(&mut store, &project);
    record_memory(&mut store, &project, "Caller variants.");

    // Two searches: one from CLI, one from MCP.
    search_lexical(&store, &project, "caller", None, 10, Caller::Cli).unwrap();
    search_lexical(&store, &project, "variants", None, 10, Caller::Mcp).unwrap();

    let callers: Vec<String> = store
        .connection()
        .prepare("SELECT caller FROM query_events WHERE project_id = ?1 ORDER BY created_at ASC")
        .unwrap()
        .query_map(rusqlite::params![project.as_str()], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(callers, vec!["cli", "mcp"]);
}
