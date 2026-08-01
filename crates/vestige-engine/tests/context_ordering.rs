//! Regression test for context-pack section ordering (#133 step 3.3, commit 2).
//!
//! `get_project_context` switched the decisions / open_questions / recent
//! sections from `ListOrder::RecencyDesc` to `ListOrder::ImportanceUsageRecency`
//! (summary stays on `RecencyDesc` — `limit: Some(1)` makes order moot there).
//! This proves the reorder actually took effect: a high-importance memory that
//! hasn't been touched in years must outrank a low-importance memory that was
//! just updated and recalled constantly — the opposite of what `RecencyDesc`
//! would produce. Without this test the ordering switch could regress silently;
//! per `crates/vestige-store/src/memory_ops/list_search.rs`,
//! `ImportanceUsageRecency` is `ORDER BY importance DESC, recall_count DESC,
//! datetime(updated_at) DESC` — importance is the primary key, so it alone
//! must flip the two seeded decisions' relative order.

use tempfile::TempDir;

use vestige_config::TracesConfig;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_engine::context::get_project_context;
use vestige_engine::Caller;
use vestige_store::Store;

// === HELPERS ===

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_project(store: &mut Store, project_id: &ProjectId) {
    store
        .ensure_project(project_id, "Context Ordering Test Project", None, None)
        .unwrap();
}

fn record_decision(
    store: &mut Store,
    project_id: &ProjectId,
    body: &str,
    importance: f64,
) -> MemoryId {
    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: MemoryType::Decision,
            body,
            importance,
            source: None,
        },
    )
    .unwrap();
    let id = bundle.memory.id.clone();
    store.record_memory(&bundle).unwrap();
    id
}

/// Backdate `updated_at` and set `recall_count` directly via SQL.
///
/// `build_bundle` always stamps "now" and there is no store API to backdate a
/// memory (correctly so — it's not a real operation). This is a test-only
/// escape hatch to simulate age and usage history deterministically.
fn backdate_and_set_recall_count(
    store: &Store,
    id: &MemoryId,
    updated_at_rfc3339: &str,
    recall_count: i64,
) {
    store
        .connection()
        .execute(
            "UPDATE memories SET updated_at = ?2, recall_count = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), updated_at_rfc3339, recall_count],
        )
        .unwrap();
}

// === TESTS ===

#[test]
fn importance_usage_recency_ranks_high_importance_old_decision_above_low_importance_recent_one() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("ctx-order-importance");
    seed_project(&mut store, &project);

    // High importance, old, never recalled. RecencyDesc would sink this to
    // the bottom; ImportanceUsageRecency must float it to the top.
    let important_old = record_decision(
        &mut store,
        &project,
        "We standardised on SQLite with FTS5 for local-first search.",
        0.95,
    );
    backdate_and_set_recall_count(&store, &important_old, "2020-01-01T00:00:00Z", 0);

    // Low importance, just touched, recalled constantly. RecencyDesc would
    // rank this first; ImportanceUsageRecency must rank it second because
    // importance is the primary sort key.
    let trivial_recent = record_decision(
        &mut store,
        &project,
        "Renamed a local variable for clarity in a debug script.",
        0.05,
    );
    backdate_and_set_recall_count(&store, &trivial_recent, "2026-08-01T00:00:00Z", 50);

    let outcome = get_project_context(
        &store,
        &project,
        "Context Ordering Test",
        10,
        4000,
        Caller::Cli,
        &TracesConfig::default(),
    )
    .unwrap();

    let ordered_ids: Vec<MemoryId> = outcome
        .pack
        .sections
        .decisions
        .iter()
        .map(|card| card.id.clone())
        .collect();

    assert_eq!(
        ordered_ids,
        vec![important_old, trivial_recent],
        "high-importance old decision must outrank the low-importance recently-touched one \
         under ImportanceUsageRecency"
    );
}
