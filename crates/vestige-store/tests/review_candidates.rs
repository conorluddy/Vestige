//! Hygiene predicates, stable ordering, and listing without side effects.

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

fn seed(
    store: &mut Store,
    project: &ProjectId,
    memory_type: MemoryType,
    importance: f64,
    age_days: u32,
) -> MemoryId {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: memory_type,
            body: "A durable fixture for review.",
            importance,
            source: None,
        },
    )
    .unwrap();
    let id = bundle.memory.id.clone();
    store.record_memory(&bundle).unwrap();
    store.connection().execute(
        "UPDATE memories SET created_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?2) WHERE id = ?1",
        rusqlite::params![id.as_str(), format!("-{age_days} days")],
    ).unwrap();
    id
}

#[test]
fn review_requires_old_unused_low_importance_active_nonprotected_memories() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    let project = ProjectId::from_slug("review");
    let other = ProjectId::from_slug("other");
    store
        .ensure_project(&project, "Review", None, None)
        .unwrap();
    store.ensure_project(&other, "Other", None, None).unwrap();

    let newer = seed(&mut store, &project, MemoryType::Note, 0.3, 40);
    let older = seed(&mut store, &project, MemoryType::Observation, 0.1, 100);
    for memory_type in [MemoryType::Decision, MemoryType::ProjectSummary] {
        seed(&mut store, &project, memory_type, 0.1, 100);
    }
    seed(&mut store, &project, MemoryType::Note, 0.3, 29);
    seed(&mut store, &project, MemoryType::Note, 0.5, 40);
    seed(&mut store, &other, MemoryType::Note, 0.1, 100);
    let recalled = seed(&mut store, &project, MemoryType::Note, 0.3, 40);
    store.bump_recall_stats(&[recalled]).unwrap();
    let expanded = seed(&mut store, &project, MemoryType::Note, 0.3, 40);
    store.bump_expand_stats(&expanded).unwrap();
    let deleted = seed(&mut store, &project, MemoryType::Note, 0.3, 40);
    store.forget_memory(&deleted).unwrap();

    let changes = store.connection().total_changes();
    let first = store.list_review_candidates(&project, 30, 0.5).unwrap();
    let second = store.list_review_candidates(&project, 30, 0.5).unwrap();
    let ids = |rows: &[vestige_core::FetchedMemory]| {
        rows.iter().map(|m| m.memory.id.clone()).collect::<Vec<_>>()
    };
    assert_eq!(ids(&first), vec![older, newer]);
    assert_eq!(ids(&first), ids(&second));
    assert_eq!(store.connection().total_changes(), changes);
    // Even maximally permissive thresholds never admit protected types.
    assert!(store
        .list_review_candidates(&project, 0, 1.0)
        .unwrap()
        .iter()
        .all(|m| {
            !matches!(
                m.memory.r#type,
                MemoryType::Decision | MemoryType::ProjectSummary
            )
        }));
}
