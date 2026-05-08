//! MCP smoke tests for M6: `vestige_expand depth=provenance` and `vestige_trace`.
//!
//! Calls each tool's `pub async fn` directly (no stdio framing). Mirrors the
//! harness style in `candidate_tools.rs` — same `make_server` helper, same
//! `envelope` / `error_body` extractors.
//!
//! PRD references: §10.2 (vestige_expand depth=provenance), §10.3 (vestige_trace),
//! §15 M6.

use rmcp::{handler::server::wrapper::Parameters, ServerHandler};
use serde_json::Value;
use tempfile::TempDir;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::{build_bundle, CandidateId, MemoryId, MemoryType, NewMemory, ProjectId};
use vestige_engine::{approve_candidate, ApprovalOverrides};
use vestige_mcp::{ExpandParams, SearchParams, TraceParams, VestigeServer};
use vestige_store::Store;

// === HELPERS ===

fn make_server(slug: &str) -> (TempDir, VestigeServer, ProjectId) {
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug(slug);

    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "M6 smoke test", None, None)
        .unwrap();

    let config: VestigeConfig = build_init_config(&project_id, "M6 smoke test", &storage_path);
    let server = VestigeServer::new(store, config, project_id.clone(), false);
    (tmp, server, project_id)
}

/// Pull the JSON envelope out of a successful `CallToolResult`.
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

/// Parse the structured `{code, message, retryable}` body from an `ErrorData`.
fn error_body(err: &rmcp::ErrorData) -> Value {
    serde_json::from_str(&err.message).expect("err.message must carry the structured body JSON")
}

/// Seed a memory through a second store connection, bypassing the MCP server's Mutex.
fn seed_memory(
    storage_path: &std::path::Path,
    project: &ProjectId,
    body: &str,
    memory_type: MemoryType,
) -> MemoryId {
    let mut store = Store::open(storage_path).unwrap();
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: memory_type,
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

/// Perform a vestige_search through the MCP server to generate a trace row.
async fn do_search(server: &VestigeServer, query: &str) {
    server
        .vestige_search(Parameters(SearchParams {
            query: query.to_string(),
            mode: Some("lexical".to_string()),
            limit: 8,
            r#type: None,
            include_score_parts: None,
        }))
        .await
        .expect("search must succeed");
}

// === vestige_expand depth=provenance ===

/// A directly-recorded memory (no candidate back-reference) must return a
/// provenance walk with at least the `memory.recorded` event.
#[tokio::test]
async fn expand_provenance_directly_recorded_memory() {
    let (tmp, server, project) = make_server("expand-prov-direct");
    let storage_path = tmp.path().join("memory.sqlite");
    let mem_id = seed_memory(
        &storage_path,
        &project,
        "direct memory body",
        MemoryType::Note,
    );

    let result = server
        .vestige_expand(Parameters(ExpandParams {
            memory_id: mem_id.to_string(),
            depth: "provenance".to_string(),
        }))
        .await
        .expect("expand depth=provenance must succeed");

    let env = envelope(&result);
    assert_eq!(
        env["memory_id"].as_str(),
        Some(mem_id.as_str()),
        "memory_id must match, got: {env}"
    );
    assert_eq!(
        env["status"], "active",
        "status must be active for a freshly recorded memory, got: {env}"
    );

    let events = env["provenance"]["events"]
        .as_array()
        .expect("provenance.events must be an array");
    assert!(
        !events.is_empty(),
        "provenance.events must contain at least one event, got: {env}"
    );
    let first_event_type = events[0]["type"]
        .as_str()
        .expect("event.type must be a string");
    assert_eq!(
        first_event_type, "memory.recorded",
        "first event must be memory.recorded, got: {first_event_type}"
    );

    // No candidate back-reference for a directly-recorded memory.
    assert!(
        env["provenance"]["candidate"].is_null(),
        "directly-recorded memory must not have a candidate back-reference, got: {env}"
    );
}

/// A candidate-promoted memory must include a candidate back-reference with its
/// own journal events.
#[tokio::test]
async fn expand_provenance_candidate_promoted_memory() {
    use vestige_core::{NewCandidate, NewCandidateSource};
    use vestige_engine::propose_candidate;

    let (tmp, server, project) = make_server("expand-prov-candidate");
    let storage_path = tmp.path().join("memory.sqlite");

    // Propose + approve a candidate through a second store connection.
    let (mem_id, cand_id) = {
        let mut store = Store::open(&storage_path).unwrap();

        let outcome = propose_candidate(
            &mut store,
            &project,
            NewCandidate {
                project_id: project.clone(),
                proposed_type: MemoryType::Decision,
                body: "promoted candidate body".to_string(),
                rationale: Some("test rationale".to_string()),
                title_override: None,
                importance: 0.7,
                confidence: 0.9,
                source: Some(NewCandidateSource {
                    source_type: "agent_session".to_string(),
                    source_ref: Some("session:abc".to_string()),
                    source_content: Some("session content".to_string()),
                }),
                duplicate_of_memory_id: None,
                duplicate_of_candidate_id: None,
            },
        )
        .expect("propose must succeed");

        let cand_id: CandidateId = outcome.candidate_id.clone();

        let approval =
            approve_candidate(&mut store, &project, &cand_id, ApprovalOverrides::default())
                .expect("approve must succeed");

        (approval.memory_id, cand_id)
    };

    let result = server
        .vestige_expand(Parameters(ExpandParams {
            memory_id: mem_id.to_string(),
            depth: "provenance".to_string(),
        }))
        .await
        .expect("expand depth=provenance must succeed for promoted memory");

    let env = envelope(&result);
    assert_eq!(
        env["memory_id"].as_str(),
        Some(mem_id.as_str()),
        "memory_id must match, got: {env}"
    );

    // The candidate back-reference must be present.
    let candidate = &env["provenance"]["candidate"];
    assert!(
        candidate.is_object(),
        "candidate back-reference must be present for promoted memory, got: {env}"
    );
    assert_eq!(
        candidate["candidate_id"].as_str(),
        Some(cand_id.as_str()),
        "candidate_id must match, got: {candidate}"
    );

    let cand_events = candidate["events"]
        .as_array()
        .expect("candidate.events must be an array");
    assert!(
        cand_events
            .iter()
            .any(|e| e["type"].as_str() == Some("candidate.proposed")),
        "candidate events must contain candidate.proposed, got: {cand_events:?}"
    );
    assert!(
        cand_events
            .iter()
            .any(|e| e["type"].as_str() == Some("candidate.approved")),
        "candidate events must contain candidate.approved, got: {cand_events:?}"
    );
}

/// Soft-deleted memory must still return a provenance walk including
/// the `memory.forgotten` event.
#[tokio::test]
async fn expand_provenance_soft_deleted_memory() {
    let (tmp, server, project) = make_server("expand-prov-deleted");
    let storage_path = tmp.path().join("memory.sqlite");
    let mem_id = seed_memory(
        &storage_path,
        &project,
        "memory to be soft-deleted",
        MemoryType::Note,
    );

    // Forget the memory through a second store connection.
    {
        let mut store = Store::open(&storage_path).unwrap();
        store.forget_memory(&mem_id).expect("forget must succeed");
    }

    let result = server
        .vestige_expand(Parameters(ExpandParams {
            memory_id: mem_id.to_string(),
            depth: "provenance".to_string(),
        }))
        .await
        .expect("expand depth=provenance must succeed even for deleted memories");

    let env = envelope(&result);
    assert_eq!(
        env["status"], "deleted",
        "status must be deleted, got: {env}"
    );

    let events = env["provenance"]["events"]
        .as_array()
        .expect("provenance.events must be an array");
    assert!(
        events
            .iter()
            .any(|e| e["type"].as_str() == Some("memory.forgotten")),
        "provenance.events must contain memory.forgotten for a soft-deleted memory, got: {events:?}"
    );
}

/// Backward compat: existing `depth=summary` must still work.
#[tokio::test]
async fn expand_depth_summary_still_works() {
    let (tmp, server, project) = make_server("expand-depth-compat");
    let storage_path = tmp.path().join("memory.sqlite");
    let mem_id = seed_memory(
        &storage_path,
        &project,
        "backward compat test body",
        MemoryType::Note,
    );

    let result = server
        .vestige_expand(Parameters(ExpandParams {
            memory_id: mem_id.to_string(),
            depth: "summary".to_string(),
        }))
        .await
        .expect("expand depth=summary must continue to work");

    let env = envelope(&result);
    assert_eq!(
        env["depth"], "summary",
        "depth field must be summary, got: {env}"
    );
    assert!(env["content"].is_string(), "content must be a string");
}

// === vestige_trace action=list ===

/// After a `vestige_search` call (which writes a trace row with `caller=mcp`),
/// `vestige_trace action=list` must return it with caller=mcp.
/// This verifies M2's MCP tracing is wired end-to-end.
#[tokio::test]
async fn trace_list_shows_mcp_search_trace() {
    let (tmp, server, project) = make_server("trace-list-mcp");
    seed_memory(
        &tmp.path().join("memory.sqlite"),
        &project,
        "trace list test memory",
        MemoryType::Note,
    );

    // Drive a search through MCP — should write a trace row with caller=mcp.
    do_search(&server, "trace list").await;

    let result = server
        .vestige_trace(Parameters(TraceParams {
            action: "list".to_string(),
            trace_id: None,
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("trace list must succeed");

    let env = envelope(&result);
    let traces = env["traces"].as_array().expect("traces must be an array");
    assert!(
        !traces.is_empty(),
        "trace list must contain at least one entry after a search, got: {env}"
    );

    let trace = &traces[0];
    assert_eq!(
        trace["caller"].as_str(),
        Some("mcp"),
        "trace must be tagged caller=mcp, got: {trace}"
    );
    assert_eq!(
        trace["kind"].as_str(),
        Some("search"),
        "trace kind must be search, got: {trace}"
    );
    assert!(
        trace["trace_id"]
            .as_str()
            .unwrap_or("")
            .starts_with("trace_"),
        "trace_id must start with trace_, got: {trace}"
    );
}

/// Filter by `caller=mcp` — must only return MCP traces.
#[tokio::test]
async fn trace_list_filter_by_caller() {
    let (tmp, server, project) = make_server("trace-list-caller-filter");
    seed_memory(
        &tmp.path().join("memory.sqlite"),
        &project,
        "caller filter test",
        MemoryType::Note,
    );

    do_search(&server, "caller filter").await;

    // Filter caller=mcp → should find our entry.
    let mcp_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "list".to_string(),
            trace_id: None,
            limit: 10,
            kind: None,
            caller: Some("mcp".to_string()),
            since: None,
        }))
        .await
        .expect("trace list caller=mcp must succeed");

    let mcp_env = envelope(&mcp_result);
    let mcp_traces = mcp_env["traces"].as_array().unwrap();
    assert!(
        !mcp_traces.is_empty(),
        "caller=mcp filter must return traces, got: {mcp_env}"
    );
    for t in mcp_traces {
        assert_eq!(
            t["caller"].as_str(),
            Some("mcp"),
            "all traces must have caller=mcp, got: {t}"
        );
    }

    // Filter caller=cli → should return empty (no CLI traces in this session).
    let cli_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "list".to_string(),
            trace_id: None,
            limit: 10,
            kind: None,
            caller: Some("cli".to_string()),
            since: None,
        }))
        .await
        .expect("trace list caller=cli must succeed");

    let cli_env = envelope(&cli_result);
    let cli_traces = cli_env["traces"].as_array().unwrap();
    assert!(
        cli_traces.is_empty(),
        "caller=cli filter must return no traces when only MCP searches were done, got: {cli_env}"
    );
}

// === vestige_trace action=show ===

/// After a search, `show` must return the full `TraceDetail` for that trace.
#[tokio::test]
async fn trace_show_returns_detail() {
    let (tmp, server, project) = make_server("trace-show");
    seed_memory(
        &tmp.path().join("memory.sqlite"),
        &project,
        "trace show test memory",
        MemoryType::Note,
    );

    do_search(&server, "trace show query").await;

    // List to get the trace_id.
    let list_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "list".to_string(),
            trace_id: None,
            limit: 1,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("list must succeed");

    let list_env = envelope(&list_result);
    let trace_id = list_env["traces"][0]["trace_id"]
        .as_str()
        .expect("trace_id must be a string");

    // Show it.
    let show_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "show".to_string(),
            trace_id: Some(trace_id.to_string()),
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("trace show must succeed");

    let show_env = envelope(&show_result);
    assert_eq!(
        show_env["trace_id"].as_str(),
        Some(trace_id),
        "show must return the requested trace, got: {show_env}"
    );
    assert_eq!(
        show_env["kind"].as_str(),
        Some("search"),
        "trace kind must be search, got: {show_env}"
    );
    assert_eq!(
        show_env["caller"].as_str(),
        Some("mcp"),
        "trace caller must be mcp, got: {show_env}"
    );
    assert!(
        show_env["query"].as_str().is_some(),
        "show must include query text, got: {show_env}"
    );
}

// === vestige_trace action=replay ===

/// Replay a stored search trace — must return `ReplayResult`-shaped envelope,
/// write a new query_events row with `caller=mcp` and `params_json.replay_of`.
#[tokio::test]
async fn trace_replay_round_trip() {
    let (tmp, server, project) = make_server("trace-replay");
    let storage_path = tmp.path().join("memory.sqlite");
    seed_memory(
        &storage_path,
        &project,
        "replay round trip test memory",
        MemoryType::Note,
    );

    // Search to create a trace.
    do_search(&server, "replay round trip").await;

    // List to get the original trace_id.
    let list_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "list".to_string(),
            trace_id: None,
            limit: 1,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("list must succeed");

    let list_env = envelope(&list_result);
    let original_trace_id = list_env["traces"][0]["trace_id"]
        .as_str()
        .expect("trace_id must be a string")
        .to_string();

    // Replay it.
    let replay_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "replay".to_string(),
            trace_id: Some(original_trace_id.clone()),
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("trace replay must succeed");

    let replay_env = envelope(&replay_result);

    // Verify ReplayResult envelope shape.
    assert_eq!(
        replay_env["trace_id"].as_str(),
        Some(original_trace_id.as_str()),
        "replay must reference the original trace_id, got: {replay_env}"
    );
    assert!(
        replay_env["original"]["result_ids"].is_array(),
        "original.result_ids must be an array, got: {replay_env}"
    );
    assert!(
        replay_env["current"]["result_ids"].is_array(),
        "current.result_ids must be an array, got: {replay_env}"
    );
    assert!(
        replay_env["diff"]["added"].is_array(),
        "diff.added must be an array, got: {replay_env}"
    );
    assert!(
        replay_env["diff"]["removed"].is_array(),
        "diff.removed must be an array, got: {replay_env}"
    );
    assert!(
        replay_env["provider_match"].is_boolean(),
        "provider_match must be a boolean, got: {replay_env}"
    );
    let replay_trace_id = replay_env["replay_trace_id"]
        .as_str()
        .expect("replay_trace_id must be a string");
    assert!(
        replay_trace_id.starts_with("trace_"),
        "replay_trace_id must start with trace_, got: {replay_trace_id}"
    );
    assert_ne!(
        replay_trace_id,
        original_trace_id.as_str(),
        "replay must create a NEW trace row"
    );
    assert!(
        replay_env["corpus_drift"].is_number(),
        "corpus_drift must be present, got: {replay_env}"
    );

    // Verify the new trace row was written with caller=mcp and replay_of.
    let show_result = server
        .vestige_trace(Parameters(TraceParams {
            action: "show".to_string(),
            trace_id: Some(replay_trace_id.to_string()),
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect("show on replay trace must succeed");

    let show_env = envelope(&show_result);
    assert_eq!(
        show_env["caller"].as_str(),
        Some("mcp"),
        "replay trace must be tagged caller=mcp, got: {show_env}"
    );

    let params = &show_env["params"];
    assert!(
        params.is_object(),
        "replay trace must have params_json, got: {show_env}"
    );
    assert_eq!(
        params["replay_of"].as_str(),
        Some(original_trace_id.as_str()),
        "params.replay_of must reference the original trace, got: {params}"
    );
}

// === vestige_trace error paths ===

/// `vestige_trace action=replay` with a wrong-prefix `trace_id` must return a
/// structured error with `retryable=false`.
#[tokio::test]
async fn trace_replay_wrong_prefix_structured_error() {
    let (_tmp, server, _project) = make_server("trace-replay-bad-id");

    let err = server
        .vestige_trace(Parameters(TraceParams {
            action: "replay".to_string(),
            trace_id: Some("mem_01HZZZZZZZZZZZZZZZZZZZZZZZ".to_string()), // wrong prefix
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect_err("wrong-prefix trace_id must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["retryable"], false,
        "malformed input is not retryable, got: {body}"
    );
    // Should be INVALID_TRACE_ID (parse failure) or VALIDATION.
    let code = body["code"].as_str().expect("code must be a string");
    assert!(
        code == "INVALID_TRACE_ID" || code == "VALIDATION",
        "code must be INVALID_TRACE_ID or VALIDATION for malformed input, got: {code}"
    );
}

/// `vestige_trace action=show` with missing `trace_id` must return MISSING_PARAM.
#[tokio::test]
async fn trace_show_missing_trace_id_errors() {
    let (_tmp, server, _project) = make_server("trace-show-no-id");

    let err = server
        .vestige_trace(Parameters(TraceParams {
            action: "show".to_string(),
            trace_id: None,
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect_err("missing trace_id must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "MISSING_PARAM",
        "missing trace_id must produce MISSING_PARAM, got: {body}"
    );
    assert_eq!(body["retryable"], false);
}

/// `vestige_trace` with an unknown `action` must return INVALID_ACTION.
#[tokio::test]
async fn trace_invalid_action_errors() {
    let (_tmp, server, _project) = make_server("trace-bad-action");

    let err = server
        .vestige_trace(Parameters(TraceParams {
            action: "obliterate".to_string(),
            trace_id: None,
            limit: 10,
            kind: None,
            caller: None,
            since: None,
        }))
        .await
        .expect_err("unknown action must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "INVALID_ACTION",
        "unknown action must produce INVALID_ACTION, got: {body}"
    );
    assert_eq!(body["retryable"], false);
}

/// Exactly ONE new tool registered: `vestige_trace`.
#[tokio::test]
async fn vestige_trace_tool_registered_exactly_once() {
    let (_tmp, server, _project) = make_server("trace-registered");

    assert!(
        server.get_tool("vestige_trace").is_some(),
        "vestige_trace must be registered"
    );

    // Confirm no spurious split-out tools were added.
    assert!(
        server.get_tool("vestige_trace_list").is_none(),
        "vestige_trace_list must NOT exist — one tool with action dispatch"
    );
    assert!(
        server.get_tool("vestige_trace_show").is_none(),
        "vestige_trace_show must NOT exist"
    );
    assert!(
        server.get_tool("vestige_trace_replay").is_none(),
        "vestige_trace_replay must NOT exist"
    );
}
