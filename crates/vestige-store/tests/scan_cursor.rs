//! Integration tests for the session_scan_cursors store API.
//!
//! Covers the three core invariants:
//! 1. Round-trip: record a cursor and read it back with correct fields.
//! 2. Missing cursor: unknown (source, file_path) returns None.
//! 3. Update in place: INSERT OR REPLACE keeps one row and advances the offset.
//!
//! All tests use real SQLite in a TempDir — no mocks.

use tempfile::TempDir;
use vestige_core::ProjectId;
use vestige_store::Store;

// === TEST HELPERS ===

fn open_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    (tmp, store)
}

fn seed_project(store: &mut Store, slug: &str) -> ProjectId {
    let project = ProjectId::from_slug(slug);
    store.ensure_project(&project, "Test", None, None).unwrap();
    project
}

// === TESTS ===

#[test]
fn record_and_get_cursor_round_trips() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "test-project");

    store
        .record_scan_cursor(
            "claude_code",
            "/repo/.claude/sessions/abc.jsonl",
            &project,
            4096,
        )
        .unwrap();

    let cursor = store
        .get_scan_cursor("claude_code", "/repo/.claude/sessions/abc.jsonl")
        .unwrap()
        .expect("cursor should exist");

    assert_eq!(cursor.source, "claude_code");
    assert_eq!(cursor.file_path, "/repo/.claude/sessions/abc.jsonl");
    assert_eq!(cursor.project_id.as_str(), project.as_str());
    assert_eq!(cursor.last_offset, 4096);
    // last_scanned_at should be a non-empty RFC-3339 string
    assert!(!cursor.last_scanned_at.is_empty());
}

#[test]
fn get_cursor_returns_none_for_unknown_pair() {
    let (_tmp, store) = open_store();

    let result = store
        .get_scan_cursor("claude_code", "/repo/.claude/sessions/does_not_exist.jsonl")
        .unwrap();

    assert!(result.is_none());
}

#[test]
fn record_cursor_again_updates_in_place() {
    let (_tmp, mut store) = open_store();
    let project = seed_project(&mut store, "update-test");
    let source = "codex";
    let path = "/repo/.openai/codex_sessions/run1.jsonl";

    // First scan: offset 1024.
    store
        .record_scan_cursor(source, path, &project, 1024)
        .unwrap();

    // Second scan: offset advances to 8192.
    store
        .record_scan_cursor(source, path, &project, 8192)
        .unwrap();

    // Only one row should exist, with the updated offset.
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM session_scan_cursors WHERE source = ?1 AND file_path = ?2",
            rusqlite::params![source, path],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "INSERT OR REPLACE must keep exactly one row");

    let cursor = store.get_scan_cursor(source, path).unwrap().unwrap();
    assert_eq!(
        cursor.last_offset, 8192,
        "watermark should advance to the latest offset"
    );
}
