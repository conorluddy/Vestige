//! MCP smoke tests for `vestige_scan_sessions`.
//!
//! Calls the tool's `pub async fn` directly (no stdio framing) and asserts the
//! response / error envelope shape. The batching, cursor, redaction, and
//! project-scope logic is unit-tested in `tools/scan_sessions.rs` against a fake
//! source; these tests cover the MCP wiring: the opt-in gate and the success
//! envelope on an empty corpus.

use rmcp::handler::server::wrapper::Parameters;
use serde_json::Value;
use tempfile::TempDir;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::ProjectId;
use vestige_mcp::{ScanSessionsParams, VestigeServer};
use vestige_store::Store;

// === HELPERS ===

fn make_server(slug: &str, allow_scan_sessions: bool) -> (TempDir, VestigeServer, ProjectId) {
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug(slug);

    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "scan smoke test", None, None)
        .unwrap();

    let mut config: VestigeConfig =
        build_init_config(&project_id, "scan smoke test", &storage_path);
    config.mcp.allow_scan_sessions = allow_scan_sessions;

    let server = VestigeServer::new(store, config, project_id.clone(), false);
    (tmp, server, project_id)
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
    let (_tmp, server, _project) = make_server("scan-disabled", false);

    let err = server
        .vestige_scan_sessions(Parameters(ScanSessionsParams { max_turns: 100 }))
        .await
        .expect_err("scan should be disabled by default");

    let body = error_body(&err);
    assert_eq!(body["code"], "SCAN_DISABLED");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn read_only_server_disables_scan_even_when_allowed() {
    // read_only must take precedence over allow_scan_sessions — the tool advances
    // scan cursors, which is a DB write.
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug("scan-readonly");
    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "scan smoke test", None, None)
        .unwrap();
    let mut config: VestigeConfig =
        build_init_config(&project_id, "scan smoke test", &storage_path);
    config.mcp.allow_scan_sessions = true;
    let server = VestigeServer::new(store, config, project_id, true);

    let err = server
        .vestige_scan_sessions(Parameters(ScanSessionsParams { max_turns: 100 }))
        .await
        .expect_err("read-only server should disable scan");

    let body = error_body(&err);
    assert_eq!(body["code"], "READ_ONLY");
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn enabled_empty_corpus_returns_empty_envelope() {
    let (_tmp, server, _project) = make_server("scan-empty", true);

    // Point the Claude Code adapter at an empty root so discovery finds nothing.
    let empty_root = TempDir::new().unwrap();
    std::env::set_var("VESTIGE_CLAUDE_ROOT", empty_root.path());

    let result = server
        .vestige_scan_sessions(Parameters(ScanSessionsParams { max_turns: 50 }))
        .await
        .expect("scan should succeed on an empty corpus");

    std::env::remove_var("VESTIGE_CLAUDE_ROOT");

    let env = envelope(&result);
    assert_eq!(env["turns_returned"], 0);
    assert_eq!(env["sessions_scanned"], 0);
    assert_eq!(env["cursor_advanced"], false);
    assert!(env["turns"].as_array().unwrap().is_empty());
}
