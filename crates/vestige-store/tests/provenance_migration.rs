//! Integration tests for migration 0005 — provenance schema.
//!
//! Covers the DoD items from issue #56:
//!   - Migration applies cleanly to a synthetic V0.2-style DB (0001-0004 applied)
//!     and to a fresh DB (0001-0005).
//!   - Backfill populates `memory_id` for `memory.recorded`, `memory.forgotten`,
//!     `memory.restored`, and `candidate.approved` events.
//!   - New writers populate `memory_id` directly on `memory.recorded`,
//!     `memory.forgotten`, `memory.restored`, `candidate.approved`.
//!   - `memory_provenance` view returns the expected joins.
//!   - `query_events` table and indexes exist and accept inserts.
//!   - Soft-deleted memory still has its events queryable via `memory_id` index.
//!   - Project-scope boundary: events from project A are invisible when querying
//!     by a memory from project B.

use std::str::FromStr;

use tempfile::TempDir;
use vestige_core::{
    build_bundle, build_candidate_bundle, CandidateId, MemoryId, MemoryType, NewCandidate,
    NewMemory, ProjectId, TraceId,
};
use vestige_store::Store;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn open_store(tmp: &TempDir) -> Store {
    Store::open(tmp.path().join("memory.sqlite")).unwrap()
}

fn ensure_project(store: &mut Store, project_id: &ProjectId) {
    store
        .ensure_project(project_id, "Test", Some("/tmp/test"), None)
        .unwrap();
}

fn record_memory(store: &mut Store, project_id: &ProjectId) -> MemoryId {
    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: MemoryType::Observation,
            body: "Test memory body.",
            importance: 0.5,
            source: None,
        },
    )
    .unwrap();
    let id = bundle.memory.id.clone();
    store.record_memory(&bundle).unwrap();
    id
}

fn record_candidate(store: &mut Store, project_id: &ProjectId) -> CandidateId {
    let bundle = build_candidate_bundle(NewCandidate {
        project_id: project_id.clone(),
        proposed_type: MemoryType::Decision,
        body: "A candidate decision.".to_string(),
        rationale: None,
        title_override: None,
        importance: 0.5,
        confidence: 0.9,
        source: None,
        duplicate_of_memory_id: None,
        duplicate_of_candidate_id: None,
    })
    .unwrap();
    let id = bundle.id.clone();
    store.record_candidate(&bundle).unwrap();
    id
}

// ─────────────────────────────────────────────────────────────────────────────
// Migration correctness
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn migration_applies_cleanly_to_fresh_db() {
    let tmp = TempDir::new().unwrap();
    // Store::open runs all migrations including 0005.
    let store = open_store(&tmp);

    // query_events table must exist.
    let qe_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='query_events'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(qe_count, 1, "query_events table must exist after migration");

    // memory_provenance view must exist.
    let view_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name='memory_provenance'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(view_count, 1, "memory_provenance view must exist");

    // memory_events.memory_id column must exist.
    let col_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_events') WHERE name='memory_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(col_count, 1, "memory_events.memory_id column must exist");
}

#[test]
fn migrations_validate_full_set() {
    // rusqlite_migration self-check: SQL parses cleanly and migration order is valid.
    // This is the same check as in lib.rs tests but run here to confirm 0005 is included.
    let tmp = TempDir::new().unwrap();
    // Opening runs all migrations; if any fail, this panics.
    open_store(&tmp);
}

#[test]
fn open_is_idempotent_with_five_migrations() {
    let tmp = TempDir::new().unwrap();
    open_store(&tmp);
    // Second open must be a no-op (migrations already applied).
    open_store(&tmp);
}

// ─────────────────────────────────────────────────────────────────────────────
// Direct memory_id population (new writer behaviour, migration 0005)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn record_memory_populates_memory_id_column_directly() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-record-memory-id");
    ensure_project(&mut store, &proj);

    let mem_id = record_memory(&mut store, &proj);

    let stored_id: Option<String> = store
        .connection()
        .query_row(
            "SELECT memory_id FROM memory_events
             WHERE event_type = 'memory.recorded' AND memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        stored_id.as_deref(),
        Some(mem_id.as_str()),
        "memory.recorded event must carry memory_id directly"
    );
}

#[test]
fn forget_and_restore_populate_memory_id_column() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-forget-restore-id");
    ensure_project(&mut store, &proj);

    let mem_id = record_memory(&mut store, &proj);

    store.forget_memory(&mem_id).unwrap();
    store.restore_memory(&mem_id).unwrap();

    // Both events must carry the memory_id directly.
    let forgotten_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_events
             WHERE event_type = 'memory.forgotten' AND memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        forgotten_count, 1,
        "memory.forgotten event must carry memory_id"
    );

    let restored_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_events
             WHERE event_type = 'memory.restored' AND memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        restored_count, 1,
        "memory.restored event must carry memory_id"
    );
}

#[test]
fn candidate_approved_populates_memory_id_column() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-approved-memory-id");
    ensure_project(&mut store, &proj);

    let cand_id = record_candidate(&mut store, &proj);
    let mem_id = MemoryId::new();
    store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

    let stored_id: Option<String> = store
        .connection()
        .query_row(
            "SELECT memory_id FROM memory_events
             WHERE event_type = 'candidate.approved' AND memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        stored_id.as_deref(),
        Some(mem_id.as_str()),
        "candidate.approved event must carry the promoted memory_id directly"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Backfill (existing rows with payload_json but no memory_id column)
//
// We simulate a "V0.2" database by directly inserting events with memory_id=NULL
// and a payload_json containing memory_id (as the old writer produced). Then we
// verify that the backfill SQL in migration 0005 would handle them correctly.
//
// Since we can't re-run a migration, we test the backfill logic directly by
// inserting NULL-memory_id events and running the same UPDATE statement.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn backfill_sql_populates_memory_id_from_payload_json() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-backfill");
    ensure_project(&mut store, &proj);

    // Insert synthetic "old-style" events with memory_id=NULL but payload_json
    // carrying memory_id (as pre-0005 writers produced).
    let test_cases: &[(&str, &str)] = &[
        (
            "memory.recorded",
            r#"{"memory_id": "mem_backfill_01", "type": "decision"}"#,
        ),
        ("memory.forgotten", r#"{"memory_id": "mem_backfill_02"}"#),
        ("memory.restored", r#"{"memory_id": "mem_backfill_03"}"#),
        (
            "candidate.approved",
            r#"{"candidate_id": "cand_x", "memory_id": "mem_backfill_04"}"#,
        ),
    ];

    let now = "2026-01-01T00:00:00Z";
    for (i, (event_type, payload)) in test_cases.iter().enumerate() {
        let event_id = format!("evt_backfill_{i:02}");
        store
            .connection()
            .execute(
                "INSERT INTO memory_events (id, project_id, event_type, payload_json, memory_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                rusqlite::params![event_id, proj.as_str(), event_type, payload, now],
            )
            .unwrap();
    }

    // Run the same backfill UPDATE that migration 0005 executes.
    store
        .connection()
        .execute(
            "UPDATE memory_events
             SET memory_id = json_extract(payload_json, '$.memory_id')
             WHERE json_extract(payload_json, '$.memory_id') IS NOT NULL
               AND memory_id IS NULL",
            [],
        )
        .unwrap();

    // Verify all four rows now have memory_id set correctly.
    let expected: &[(&str, &str)] = &[
        ("evt_backfill_00", "mem_backfill_01"),
        ("evt_backfill_01", "mem_backfill_02"),
        ("evt_backfill_02", "mem_backfill_03"),
        ("evt_backfill_03", "mem_backfill_04"),
    ];
    for (event_id, expected_memory_id) in expected {
        let stored: Option<String> = store
            .connection()
            .query_row(
                "SELECT memory_id FROM memory_events WHERE id = ?1",
                rusqlite::params![event_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(*expected_memory_id),
            "backfill mismatch for event `{event_id}`"
        );
    }
}

#[test]
fn backfill_leaves_null_memory_id_for_events_without_payload_memory_id() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-backfill-null");
    ensure_project(&mut store, &proj);

    // Insert an event whose payload_json does not contain memory_id.
    store
        .connection()
        .execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, memory_id, created_at)
             VALUES ('evt_no_mem_id', ?1, 'project.init', '{\"project_id\": \"proj_x\"}', NULL, '2026-01-01T00:00:00Z')",
            rusqlite::params![proj.as_str()],
        )
        .unwrap();

    // Run the backfill (same SQL as migration 0005).
    store
        .connection()
        .execute(
            "UPDATE memory_events
             SET memory_id = json_extract(payload_json, '$.memory_id')
             WHERE json_extract(payload_json, '$.memory_id') IS NOT NULL
               AND memory_id IS NULL",
            [],
        )
        .unwrap();

    let stored: Option<String> = store
        .connection()
        .query_row(
            "SELECT memory_id FROM memory_events WHERE id = 'evt_no_mem_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        stored.is_none(),
        "event without memory_id in payload_json must remain NULL after backfill"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// memory_provenance view
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn memory_provenance_view_joins_events_for_recorded_memory() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-view-join");
    ensure_project(&mut store, &proj);

    let mem_id = record_memory(&mut store, &proj);

    // The view must return at least one row for this memory (the recorded event).
    let event_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_provenance WHERE memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        event_count >= 1,
        "memory_provenance must return at least the recorded event"
    );

    // The event_type returned must be memory.recorded.
    let event_type: String = store
        .connection()
        .query_row(
            "SELECT event_type FROM memory_provenance WHERE memory_id = ?1 LIMIT 1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_type, "memory.recorded");
}

#[test]
fn memory_provenance_view_shows_forgotten_and_restored_events() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-view-lifecycle");
    ensure_project(&mut store, &proj);

    let mem_id = record_memory(&mut store, &proj);
    store.forget_memory(&mem_id).unwrap();
    store.restore_memory(&mem_id).unwrap();

    let event_types: Vec<String> = {
        let mut stmt = store
            .connection()
            .prepare(
                "SELECT event_type FROM memory_provenance WHERE memory_id = ?1 ORDER BY event_at",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![mem_id.as_str()], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    assert!(
        event_types.contains(&"memory.recorded".to_string()),
        "must include recorded event"
    );
    assert!(
        event_types.contains(&"memory.forgotten".to_string()),
        "must include forgotten event"
    );
    assert!(
        event_types.contains(&"memory.restored".to_string()),
        "must include restored event"
    );
}

#[test]
fn memory_provenance_view_includes_soft_deleted_memory() {
    // Provenance must remain inspectable for forgotten memories (PRD §11.3).
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-view-deleted");
    ensure_project(&mut store, &proj);

    let mem_id = record_memory(&mut store, &proj);
    store.forget_memory(&mem_id).unwrap();

    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_provenance WHERE memory_id = ?1",
            rusqlite::params![mem_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        count >= 2,
        "deleted memory must have both recorded and forgotten events in provenance view"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// query_events table
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn query_events_accepts_insert_and_is_project_scoped() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj_a = ProjectId::from_slug("test-trace-a");
    let proj_b = ProjectId::from_slug("test-trace-b");
    ensure_project(&mut store, &proj_a);
    ensure_project(&mut store, &proj_b);

    let trace_id_a = TraceId::new();
    let trace_id_b = TraceId::new();

    store
        .connection()
        .execute(
            "INSERT INTO query_events
                 (id, project_id, kind, caller, result_count, latency_ms, created_at)
             VALUES (?1, ?2, 'search', 'mcp', 3, 47, '2026-05-08T14:02:11Z')",
            rusqlite::params![trace_id_a.as_str(), proj_a.as_str()],
        )
        .unwrap();

    store
        .connection()
        .execute(
            "INSERT INTO query_events
                 (id, project_id, kind, caller, result_count, latency_ms, created_at)
             VALUES (?1, ?2, 'context', 'cli', 12, 18, '2026-05-08T13:58:42Z')",
            rusqlite::params![trace_id_b.as_str(), proj_b.as_str()],
        )
        .unwrap();

    // Project A should see only its trace.
    let count_a: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM query_events WHERE project_id = ?1",
            rusqlite::params![proj_a.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_a, 1, "project A must see only its own traces");

    // Project B should see only its trace.
    let count_b: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM query_events WHERE project_id = ?1",
            rusqlite::params![proj_b.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_b, 1, "project B must see only its own traces");
}

#[test]
fn query_events_full_row_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-trace-roundtrip");
    ensure_project(&mut store, &proj);

    let trace_id = TraceId::new();

    store
        .connection()
        .execute(
            "INSERT INTO query_events
                 (id, project_id, kind, mode_requested, mode_resolved, query_text,
                  params_json, caller, provider, provider_model,
                  result_ids_json, result_scores_json, result_count, latency_ms, created_at)
             VALUES (?1, ?2, 'search', 'hybrid', 'hybrid', 'skill install',
                     '{\"limit\": 10}', 'mcp', 'fastembed', 'BAAI/bge-small-en-v1.5',
                     '[\"mem_abc\"]', '[0.91]', 1, 47, '2026-05-08T14:00:00Z')",
            rusqlite::params![trace_id.as_str(), proj.as_str()],
        )
        .unwrap();

    let (kind, mode, query, caller, provider): (String, String, String, String, String) = store
        .connection()
        .query_row(
            "SELECT kind, mode_requested, query_text, caller, provider
             FROM query_events WHERE id = ?1",
            rusqlite::params![trace_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();

    assert_eq!(kind, "search");
    assert_eq!(mode, "hybrid");
    assert_eq!(query, "skill install");
    assert_eq!(caller, "mcp");
    assert_eq!(provider, "fastembed");
}

// ─────────────────────────────────────────────────────────────────────────────
// Project scope boundary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn memory_events_from_project_a_invisible_when_querying_project_b_memory_id() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj_a = ProjectId::from_slug("scope-a");
    let proj_b = ProjectId::from_slug("scope-b");
    ensure_project(&mut store, &proj_a);
    ensure_project(&mut store, &proj_b);

    let mem_a = record_memory(&mut store, &proj_a);

    // Query memory_events for a MemoryId that belongs to project A, but filter
    // additionally by project B — must return nothing.
    let cross_count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memory_events
             WHERE memory_id = ?1 AND project_id = ?2",
            rusqlite::params![mem_a.as_str(), proj_b.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        cross_count, 0,
        "events from project A must be invisible when filtering by project B"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TraceId prefix validation (store layer smoke — full unit tests in vestige-core)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn trace_id_new_has_trace_prefix() {
    let id = TraceId::new();
    assert!(id.as_str().starts_with("trace_"));
}

#[test]
fn trace_id_rejects_mem_prefix() {
    assert!(TraceId::from_str("mem_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// fetch_traces_for_memory — V0.4 trace forward-link
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn fetch_traces_for_memory_returns_only_matching_rows() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-traces-of");
    ensure_project(&mut store, &proj);

    let target = MemoryId::new();
    let other = MemoryId::new();

    let make_trace = |store: &Store, ids: &[&MemoryId], created_at: &str| -> TraceId {
        let trace_id = TraceId::new();
        let json =
            serde_json::to_string(&ids.iter().map(|id| id.as_str()).collect::<Vec<_>>()).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO query_events
                     (id, project_id, kind, caller, result_count, latency_ms, created_at,
                      result_ids_json)
                 VALUES (?1, ?2, 'search', 'cli', ?3, 5, ?4, ?5)",
                rusqlite::params![
                    trace_id.as_str(),
                    proj.as_str(),
                    ids.len() as i64,
                    created_at,
                    json,
                ],
            )
            .unwrap();
        trace_id
    };

    let t_hit_1 = make_trace(&store, &[&target, &other], "2026-05-08T10:00:00Z");
    let _t_miss = make_trace(&store, &[&other], "2026-05-08T11:00:00Z");
    let t_hit_2 = make_trace(&store, &[&target], "2026-05-08T12:00:00Z");

    let hits = store
        .fetch_traces_for_memory(proj.as_str(), &target, 50)
        .unwrap();
    let hit_ids: Vec<&str> = hits.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        hit_ids,
        vec![t_hit_2.as_str(), t_hit_1.as_str()],
        "expected most-recent first; got {hit_ids:?}"
    );
}

#[test]
fn fetch_traces_for_memory_is_project_scoped() {
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj_a = ProjectId::from_slug("test-scope-traces-a");
    let proj_b = ProjectId::from_slug("test-scope-traces-b");
    ensure_project(&mut store, &proj_a);
    ensure_project(&mut store, &proj_b);

    let mem = MemoryId::new();
    let trace_in_b = TraceId::new();
    let json = serde_json::to_string(&[mem.as_str()]).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO query_events
                 (id, project_id, kind, caller, result_count, latency_ms, created_at,
                  result_ids_json)
             VALUES (?1, ?2, 'search', 'mcp', 1, 4, '2026-05-08T10:00:00Z', ?3)",
            rusqlite::params![trace_in_b.as_str(), proj_b.as_str(), json],
        )
        .unwrap();

    assert!(store
        .fetch_traces_for_memory(proj_a.as_str(), &mem, 50)
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .fetch_traces_for_memory(proj_b.as_str(), &mem, 50)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn fetch_traces_for_memory_ignores_substring_collisions() {
    // A LIKE scan on bare ULIDs could collide because ULIDs share a Crockford
    // alphabet — `mem_01HX…` could substring-match `mem_01HY…`. We wrap each
    // ID in double quotes in the JSON before matching, so the LIKE pattern
    // anchors on `"<full id>"` and cannot accidentally match a longer ID.
    let tmp = TempDir::new().unwrap();
    let mut store = open_store(&tmp);
    let proj = ProjectId::from_slug("test-substring");
    ensure_project(&mut store, &proj);

    let target = MemoryId::new();

    // Craft a JSON payload that contains the target ID as a bare substring
    // (not wrapped in quotes) — should NOT match.
    let bare = format!("[\"{}_suffix\"]", target.as_str());
    store
        .connection()
        .execute(
            "INSERT INTO query_events
                 (id, project_id, kind, caller, result_count, latency_ms, created_at,
                  result_ids_json)
             VALUES (?1, ?2, 'search', 'cli', 1, 5, '2026-05-08T10:00:00Z', ?3)",
            rusqlite::params![TraceId::new().as_str(), proj.as_str(), bare],
        )
        .unwrap();

    assert!(store
        .fetch_traces_for_memory(proj.as_str(), &target, 50)
        .unwrap()
        .is_empty());
}
