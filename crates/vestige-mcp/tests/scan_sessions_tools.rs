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
use tokio::sync::Mutex;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::ProjectId;
use vestige_mcp::{ScanSessionsParams, VestigeServer};
use vestige_store::Store;

/// Serialises tests that mutate the `VESTIGE_*_ROOT` env seams — process-global env
/// would otherwise race across cargo's parallel test threads. A `tokio::sync::Mutex`
/// (not `std`) so the guard can be held across the tool's `.await` without tripping
/// `clippy::await_holding_lock`.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

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
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, server, _project) = make_server("scan-empty", true);

    // Point both adapters at empty roots so discovery finds nothing (and never
    // touches the real ~/.claude or ~/.codex on the host).
    let empty_claude = TempDir::new().unwrap();
    let empty_codex = TempDir::new().unwrap();
    std::env::set_var("VESTIGE_CLAUDE_ROOT", empty_claude.path());
    std::env::set_var("VESTIGE_CODEX_ROOT", empty_codex.path());

    let result = server
        .vestige_scan_sessions(Parameters(ScanSessionsParams { max_turns: 50 }))
        .await
        .expect("scan should succeed on an empty corpus");

    std::env::remove_var("VESTIGE_CLAUDE_ROOT");
    std::env::remove_var("VESTIGE_CODEX_ROOT");

    let env = envelope(&result);
    assert_eq!(env["turns_returned"], 0);
    assert_eq!(env["sessions_scanned"], 0);
    assert_eq!(env["cursor_advanced"], false);
    assert!(env["turns"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn codex_session_surfaces_turns_with_codex_source() {
    let _guard = ENV_LOCK.lock().await;
    let (_tmp, server, project_id) = make_server("scan-codex", true);

    // A project dir whose .vestige/config.toml pins the SAME project_id the server serves.
    let proj_tmp = TempDir::new().unwrap();
    let project_root = proj_tmp.path().join("proj");
    std::fs::create_dir_all(&project_root).unwrap();
    let cfg = build_init_config(
        &project_id,
        "scan smoke test",
        &project_root.join("memory.sqlite"),
    );
    vestige_config::paths::write_config(&project_root.join(".vestige").join("config.toml"), &cfg)
        .unwrap();

    // Codex date-partitioned fixture; cwd lives in the session_meta record.
    let codex_root = TempDir::new().unwrap();
    let day = codex_root.path().join("2026").join("06").join("14");
    std::fs::create_dir_all(&day).unwrap();
    let jsonl = format!(
        concat!(
            r#"{{"type":"session_meta","payload":{{"id":"abc","cwd":"{cwd}"}}}}"#,
            "\n",
            r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"decide: use codex source"}}]}}}}"#,
            "\n",
        ),
        cwd = project_root.to_str().unwrap()
    );
    std::fs::write(day.join("rollout-2026-06-14T10-00-00-uuid.jsonl"), jsonl).unwrap();

    // Empty Claude root so only Codex contributes.
    let empty_claude = TempDir::new().unwrap();
    std::env::set_var("VESTIGE_CLAUDE_ROOT", empty_claude.path());
    std::env::set_var("VESTIGE_CODEX_ROOT", codex_root.path());

    let result = server
        .vestige_scan_sessions(Parameters(ScanSessionsParams { max_turns: 100 }))
        .await
        .expect("scan should succeed");

    std::env::remove_var("VESTIGE_CLAUDE_ROOT");
    std::env::remove_var("VESTIGE_CODEX_ROOT");

    let env = envelope(&result);
    let turns = env["turns"].as_array().expect("turns array");
    assert_eq!(turns.len(), 1, "expected one codex turn, got {env}");
    assert_eq!(turns[0]["source"], "codex");
    assert_eq!(turns[0]["role"], "user");
    assert_eq!(turns[0]["text"], "decide: use codex source");
    assert!(turns[0]["source_ref"]
        .as_str()
        .unwrap()
        .starts_with("codex:"));
}
