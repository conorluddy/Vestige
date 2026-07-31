//! MCP smoke tests for `vestige_revise_memory` (issue #132).
//!
//! Calls the tool's `pub async fn` directly (no stdio framing) and asserts the
//! response / error envelope shape. Mirrors the harness style in
//! `candidate_tools.rs` / `scan_sessions_tools.rs` — same `make_server`
//! helper, same `envelope` / `error_body` extractors.

use rmcp::handler::server::wrapper::Parameters;
use serde_json::Value;
use tempfile::TempDir;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_mcp::{ReviseMemoryParams, VestigeServer};
use vestige_store::Store;

// === HELPERS ===

fn make_server(
    slug: &str,
    allow_revise: bool,
) -> (TempDir, VestigeServer, ProjectId, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug(slug);

    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "revise smoke test", None, None)
        .unwrap();

    let mut config: VestigeConfig =
        build_init_config(&project_id, "revise smoke test", &storage_path);
    config.mcp.allow_revise = allow_revise;

    let server = VestigeServer::new(store, config, project_id.clone(), false);
    (tmp, server, project_id, storage_path)
}

/// Seed a memory through a second store connection, bypassing the MCP server's Mutex.
fn seed_memory(storage_path: &std::path::Path, project: &ProjectId, body: &str) -> MemoryId {
    let mut store = Store::open(storage_path).unwrap();
    let bundle = build_bundle(
        project,
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

fn envelope(result: &rmcp::model::CallToolResult) -> Value {
    let text = result
        .content
        .first()
        .expect("CallToolResult must have one content block")
        .as_text()
        .expect("content must be Text")
        .text
        .clone();
    serde_json::from_str(&text).expect("envelope must be valid JSON")
}

fn error_body(err: &rmcp::ErrorData) -> Value {
    serde_json::from_str(&err.message).expect("err.message must carry the structured body JSON")
}

// === TESTS ===

#[tokio::test]
async fn disabled_gate_returns_structured_error() {
    let (_tmp, server, project_id, storage_path) = make_server("revise-disabled", false);
    let id = seed_memory(&storage_path, &project_id, "the original body");

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("revise should be disabled by allow_revise = false");

    let body = error_body(&err);
    assert_eq!(body["code"], "REVISE_DISABLED");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn read_only_server_disables_revise_even_when_allowed() {
    // read_only must take precedence over allow_revise — check both orderings.
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug("revise-readonly");
    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "revise smoke test", None, None)
        .unwrap();
    let mut config: VestigeConfig =
        build_init_config(&project_id, "revise smoke test", &storage_path);
    config.mcp.allow_revise = true;
    let server = VestigeServer::new(store, config, project_id.clone(), true);
    let id = seed_memory(&storage_path, &project_id, "the original body");

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("read-only server should disable revise regardless of allow_revise");

    let body = error_body(&err);
    assert_eq!(body["code"], "READ_ONLY");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn read_only_wins_even_when_allow_revise_is_false_too() {
    // Same assertion, opposite starting gate — READ_ONLY must win either way.
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug("revise-readonly-2");
    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "revise smoke test", None, None)
        .unwrap();
    let mut config: VestigeConfig =
        build_init_config(&project_id, "revise smoke test", &storage_path);
    config.mcp.allow_revise = false;
    let server = VestigeServer::new(store, config, project_id.clone(), true);
    let id = seed_memory(&storage_path, &project_id, "the original body");

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("read-only server should disable revise");

    let body = error_body(&err);
    assert_eq!(body["code"], "READ_ONLY");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn enabled_and_writable_revises_and_returns_envelope() {
    let (_tmp, server, project_id, storage_path) = make_server("revise-ok", true);
    let id = seed_memory(&storage_path, &project_id, "the original body");

    let result = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "a completely revised body".to_string(),
        }))
        .await
        .expect("revise should succeed when enabled and writable");

    let env = envelope(&result);
    assert_eq!(env["memory_id"], id.as_str());
    assert!(
        env["prior_one_liner"]
            .as_str()
            .unwrap()
            .contains("original"),
        "prior_one_liner should reflect the pre-revision body, got: {env}"
    );
    assert!(
        env["new_one_liner"].as_str().unwrap().contains("revised"),
        "new_one_liner should reflect the post-revision body, got: {env}"
    );
}

#[tokio::test]
async fn invalid_id_returns_structured_parse_error() {
    let (_tmp, server, _project_id, _storage_path) = make_server("revise-invalid-id", true);

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: "not_a_valid_id".to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("malformed id must be rejected before reaching the store");

    let body = error_body(&err);
    assert_eq!(body["code"], "INVALID_ID");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn nonexistent_id_returns_structured_not_found() {
    let (_tmp, server, _project_id, _storage_path) = make_server("revise-missing", true);
    let missing_id = MemoryId::new();

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: missing_id.as_str().to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("well-formed but nonexistent id must be reported as not found");

    let body = error_body(&err);
    assert_eq!(body["code"], "MEMORY_NOT_FOUND");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn empty_body_returns_structured_validation_error() {
    let (_tmp, server, project_id, storage_path) = make_server("revise-empty-body", true);
    let id = seed_memory(&storage_path, &project_id, "the original body");

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "   ".to_string(),
        }))
        .await
        .expect_err("whitespace-only body must fail validation, not panic");

    let body = error_body(&err);
    assert_eq!(body["code"], "VALIDATION");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn deleted_memory_returns_structured_validation_error() {
    let (_tmp, server, project_id, storage_path) = make_server("revise-deleted", true);
    let id = seed_memory(&storage_path, &project_id, "the original body");
    {
        let mut store = Store::open(&storage_path).unwrap();
        assert!(store.forget_memory(&id).unwrap());
    }

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: id.as_str().to_string(),
            body: "a revised body".to_string(),
        }))
        .await
        .expect_err("a non-active memory must fail validation, not silently succeed");

    let body = error_body(&err);
    assert_eq!(body["code"], "VALIDATION");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn revise_tool_is_registered_on_the_server() {
    let (_tmp, server, _project_id, _storage_path) = make_server("revise-registered", true);

    assert!(
        server.has_tool("vestige_revise_memory"),
        "vestige_revise_memory must be registered on the tool router"
    );
}

#[tokio::test]
async fn revise_of_memory_in_another_project_is_out_of_scope() {
    let (_tmp, server, _project_id, storage_path) = make_server("revise-scope-a", true);
    let other_project = ProjectId::from_slug("revise-scope-b");
    {
        let mut store = Store::open(&storage_path).unwrap();
        store
            .ensure_project(&other_project, "other project", None, None)
            .unwrap();
    }
    let foreign_id = seed_memory(&storage_path, &other_project, "belongs to another project");

    let err = server
        .vestige_revise_memory(Parameters(ReviseMemoryParams {
            id: foreign_id.as_str().to_string(),
            body: "an attempted cross-project revision".to_string(),
        }))
        .await
        .expect_err("revising a memory from another project must be rejected");

    let body = error_body(&err);
    assert_eq!(body["code"], "OUT_OF_SCOPE");
    assert_eq!(body["retryable"], false);
}
