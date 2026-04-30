//! Integration tests for the FTS5 sync triggers and search.
//!
//! These exercise the invariants from CLAUDE.md:
//!   1. Soft-delete excludes from search.
//!   2. Restore re-indexes.
//!   4. Project-scope boundary: search in project A returns nothing from B.

use std::str::FromStr;

use tempfile::TempDir;
use vestige_core::{
    build_bundle, sanitize_fts_query, MemoryType, NewMemory, ProjectId, SearchFilter,
};
use vestige_store::Store;

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn record(store: &mut Store, project: &ProjectId, ty: MemoryType, body: &str) -> String {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: ty,
            body,
            importance: 0.5,
            source: None,
        },
    )
    .unwrap();
    let id = bundle.memory.id.to_string();
    store.record_memory(&bundle).unwrap();
    id
}

#[test]
fn soft_delete_excludes_from_search() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    let id = record(
        &mut store,
        &project,
        MemoryType::Decision,
        "Use SQLite as the canonical store.",
    );
    let id = vestige_core::MemoryId::from_str(&id).unwrap();

    let q = sanitize_fts_query("SQLite canonical");
    let hits = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(hits.len(), 1, "expected one match before forget");

    assert!(store.forget_memory(&id).unwrap());

    let hits = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert!(
        hits.is_empty(),
        "soft-deleted memory must not appear in search results"
    );
}

#[test]
fn restore_reindexes_into_search() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    let id_str = record(
        &mut store,
        &project,
        MemoryType::Note,
        "MCP is a thin adapter over the engine.",
    );
    let id = vestige_core::MemoryId::from_str(&id_str).unwrap();

    store.forget_memory(&id).unwrap();
    let q = sanitize_fts_query("adapter engine");
    assert!(store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap()
        .is_empty());

    assert!(store.restore_memory(&id).unwrap());
    let hits = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(hits.len(), 1, "restored memory must reappear in search");
    assert_eq!(hits[0].fetched.memory.id.to_string(), id_str);
}

#[test]
fn project_scope_boundary_holds() {
    let (_tmp, mut store) = open_store();
    let p_a = ProjectId::from_slug("a");
    let p_b = ProjectId::from_slug("b");
    store.ensure_project(&p_a, "A", None, None).unwrap();
    store.ensure_project(&p_b, "B", None, None).unwrap();

    record(
        &mut store,
        &p_a,
        MemoryType::Decision,
        "Project A: use SQLite.",
    );
    record(
        &mut store,
        &p_b,
        MemoryType::Note,
        "Project B: also uses SQLite.",
    );

    let q = sanitize_fts_query("SQLite");

    let hits_a = store
        .search_memories(&p_a, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(hits_a.len(), 1);
    assert_eq!(hits_a[0].fetched.memory.project_id, p_a);

    let hits_b = store
        .search_memories(&p_b, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(hits_b.len(), 1);
    assert_eq!(hits_b[0].fetched.memory.project_id, p_b);
}

#[test]
fn type_filter_narrows_results() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();

    record(
        &mut store,
        &project,
        MemoryType::Decision,
        "Decision: use SQLite.",
    );
    record(&mut store, &project, MemoryType::Note, "Note: use SQLite.");

    let q = sanitize_fts_query("SQLite");

    let all = store
        .search_memories(&project, &q, &SearchFilter::default())
        .unwrap();
    assert_eq!(all.len(), 2);

    let decisions = store
        .search_memories(
            &project,
            &q,
            &SearchFilter {
                r#type: Some(MemoryType::Decision),
                limit: None,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].fetched.memory.r#type, MemoryType::Decision);
}

#[test]
fn empty_query_returns_no_results() {
    let (_tmp, mut store) = open_store();
    let project = ProjectId::from_slug("p");
    store.ensure_project(&project, "P", None, None).unwrap();
    record(&mut store, &project, MemoryType::Note, "anything");

    let hits = store
        .search_memories(&project, "", &SearchFilter::default())
        .unwrap();
    assert!(hits.is_empty());
}
