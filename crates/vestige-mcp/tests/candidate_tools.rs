//! MCP smoke tests for the assimilation-inbox candidate tools:
//! `vestige_propose_candidate`, `vestige_list_candidates`, `vestige_get_candidate`.
//!
//! Calls each tool's `pub async fn` directly (no stdio framing). Mirrors the
//! harness style in `search_modes.rs` exactly — same `make_server` helper,
//! same `envelope` / `error_body` extractors.
//!
//! PRD references: §10.3, §10.4 (non-goals), §15.1.

use rmcp::{handler::server::wrapper::Parameters, ServerHandler};
use serde_json::Value;
use tempfile::TempDir;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::{CandidateId, ProjectId, RejectionReason};
use vestige_engine::{approve_candidate, reject_candidate, ApprovalOverrides};
use vestige_mcp::{
    GetCandidateParams, ListCandidatesParams, ProposeCandidateParams, ProposeSource, VestigeServer,
};
use vestige_store::Store;

// === HELPERS ===

fn make_server(slug: &str) -> (TempDir, VestigeServer, ProjectId) {
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug(slug);

    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "candidate smoke test", None, None)
        .unwrap();

    let config: VestigeConfig =
        build_init_config(&project_id, "candidate smoke test", &storage_path);
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

// === TESTS ===

#[tokio::test]
async fn propose_then_list_then_get() {
    let (_tmp, server, _project) = make_server("cand-full-journey");

    // 1. Propose
    let propose_result = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "decision".to_string(),
            title: None,
            body: "use dual skill targets".to_string(),
            rationale: Some("claude+codex".to_string()),
            importance: 0.7,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect("propose should succeed");

    let env = envelope(&propose_result);
    let candidate_id = env["candidate_id"]
        .as_str()
        .expect("candidate_id must be a string");
    assert!(
        candidate_id.starts_with("cand_"),
        "candidate_id must start with cand_, got: {candidate_id}"
    );
    assert_eq!(
        env["status"], "pending",
        "newly proposed candidate must be pending"
    );
    assert!(
        env["similar_memories"].is_array(),
        "similar_memories must be an array"
    );
    assert!(
        env["similar_candidates"].is_array(),
        "similar_candidates must be an array"
    );

    // 2. List — must include the new candidate
    let list_result = server
        .vestige_list_candidates(Parameters(ListCandidatesParams {
            status: None,
            r#type: None,
            limit: 50,
            include_rejected: false,
        }))
        .await
        .expect("list should succeed");

    let list_env = envelope(&list_result);
    let candidates = list_env["candidates"]
        .as_array()
        .expect("candidates must be an array");
    assert!(
        !candidates.is_empty(),
        "list must return at least one candidate"
    );
    let found = candidates
        .iter()
        .any(|c| c["id"].as_str() == Some(candidate_id));
    assert!(found, "proposed candidate must appear in list");

    // 3. Get — must return full row with rationale and proposed_type
    let get_result = server
        .vestige_get_candidate(Parameters(GetCandidateParams {
            candidate_id: candidate_id.to_string(),
        }))
        .await
        .expect("get should succeed");

    let get_env = envelope(&get_result);
    assert_eq!(
        get_env["id"].as_str(),
        Some(candidate_id),
        "get must return the same id"
    );
    assert_eq!(
        get_env["proposed_type"], "decision",
        "proposed_type must be decision"
    );
    assert_eq!(
        get_env["rationale"], "claude+codex",
        "rationale must be preserved"
    );
    assert_eq!(get_env["status"], "pending", "status must still be pending");
}

#[tokio::test]
async fn propose_with_source_persists_source_row() {
    let (_tmp, server, _project) = make_server("cand-source");

    let propose_result = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "note".to_string(),
            title: None,
            body: "source attachment test".to_string(),
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: Some(ProposeSource {
                r#type: "file".to_string(),
                r#ref: Some("README.md:42".to_string()),
                content: Some("snippet".to_string()),
            }),
        }))
        .await
        .expect("propose with source should succeed");

    let env = envelope(&propose_result);
    let candidate_id = env["candidate_id"]
        .as_str()
        .expect("candidate_id must be a string");

    let get_result = server
        .vestige_get_candidate(Parameters(GetCandidateParams {
            candidate_id: candidate_id.to_string(),
        }))
        .await
        .expect("get should succeed");

    let get_env = envelope(&get_result);
    let sources = get_env["sources"]
        .as_array()
        .expect("sources must be an array");
    assert!(
        !sources.is_empty(),
        "sources must contain at least one row after propose with source"
    );
    let source = &sources[0];
    assert_eq!(
        source["source_type"].as_str(),
        Some("file"),
        "source_type must be file"
    );
    assert_eq!(
        source["source_ref"].as_str(),
        Some("README.md:42"),
        "source_ref must match"
    );
    assert_eq!(
        source["source_content"].as_str(),
        Some("snippet"),
        "source_content must match"
    );
}

#[tokio::test]
async fn propose_validation_empty_body_errors() {
    let (_tmp, server, _project) = make_server("cand-empty-body");

    let err = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "decision".to_string(),
            title: None,
            body: "   ".to_string(), // whitespace-only → empty after trim
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect_err("empty body must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "VALIDATION",
        "empty body must produce VALIDATION code, got: {body}"
    );
    assert_eq!(
        body["retryable"], false,
        "validation errors are not retryable"
    );
}

#[tokio::test]
async fn propose_invalid_type_errors() {
    let (_tmp, server, _project) = make_server("cand-invalid-type");

    let err = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "garbage".to_string(),
            title: None,
            body: "some valid body".to_string(),
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect_err("unknown type must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "INVALID_TYPE",
        "unknown type must produce INVALID_TYPE code, got: {body}"
    );
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn get_candidate_invalid_id_errors() {
    let (_tmp, server, _project) = make_server("cand-bad-id");

    let err = server
        .vestige_get_candidate(Parameters(GetCandidateParams {
            candidate_id: "not_a_real_id".to_string(),
        }))
        .await
        .expect_err("invalid id must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "INVALID_CANDIDATE_ID",
        "malformed id must produce INVALID_CANDIDATE_ID, got: {body}"
    );
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn get_candidate_not_found_errors() {
    let (_tmp, server, _project) = make_server("cand-not-found");

    // Syntactically valid cand_ prefix + valid ULID, but no row in DB.
    let err = server
        .vestige_get_candidate(Parameters(GetCandidateParams {
            candidate_id: "cand_01HZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        }))
        .await
        .expect_err("absent candidate must return Err");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "CANDIDATE_NOT_FOUND",
        "absent candidate must produce CANDIDATE_NOT_FOUND, got: {body}"
    );
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn list_candidates_status_filter() {
    let (tmp, server, project) = make_server("cand-status-filter");
    let storage_path = tmp.path().join("memory.sqlite");

    // Propose two candidates via the MCP tool.
    let res_a = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "note".to_string(),
            title: None,
            body: "candidate alpha for status filter test".to_string(),
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect("propose alpha should succeed");
    let env_a = envelope(&res_a);
    let id_a: CandidateId = CandidateId::new(env_a["candidate_id"].as_str().unwrap()).unwrap();

    let _res_b = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "note".to_string(),
            title: None,
            body: "candidate beta for status filter test".to_string(),
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect("propose beta should succeed");

    // Approve candidate A via the engine directly (in-process, second connection).
    {
        let mut store = Store::open(&storage_path).unwrap();
        approve_candidate(&mut store, &project, &id_a, ApprovalOverrides::default())
            .expect("approve alpha should succeed");
    }

    // list {status: "pending"} → 1 result
    let pending_result = server
        .vestige_list_candidates(Parameters(ListCandidatesParams {
            status: Some("pending".to_string()),
            r#type: None,
            limit: 50,
            include_rejected: false,
        }))
        .await
        .expect("list pending should succeed");
    let pending_env = envelope(&pending_result);
    let pending_candidates = pending_env["candidates"].as_array().unwrap();
    assert_eq!(
        pending_candidates.len(),
        1,
        "only one candidate should be pending, got: {pending_candidates:?}"
    );

    // list {status: "approved"} → 1 result
    let approved_result = server
        .vestige_list_candidates(Parameters(ListCandidatesParams {
            status: Some("approved".to_string()),
            r#type: None,
            limit: 50,
            include_rejected: false,
        }))
        .await
        .expect("list approved should succeed");
    let approved_env = envelope(&approved_result);
    let approved_candidates = approved_env["candidates"].as_array().unwrap();
    assert_eq!(
        approved_candidates.len(),
        1,
        "only one candidate should be approved, got: {approved_candidates:?}"
    );
    assert_eq!(
        approved_candidates[0]["id"].as_str(),
        Some(id_a.as_str()),
        "approved candidate must be id_a"
    );
}

#[tokio::test]
async fn list_candidates_default_excludes_rejected() {
    let (tmp, server, project) = make_server("cand-rejected-filter");
    let storage_path = tmp.path().join("memory.sqlite");

    let res = server
        .vestige_propose_candidate(Parameters(ProposeCandidateParams {
            r#type: "note".to_string(),
            title: None,
            body: "candidate to be rejected in filter test".to_string(),
            rationale: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
        }))
        .await
        .expect("propose should succeed");
    let env = envelope(&res);
    let candidate_id = CandidateId::new(env["candidate_id"].as_str().unwrap()).unwrap();

    // Reject via engine in-process (second connection).
    {
        let mut store = Store::open(&storage_path).unwrap();
        reject_candidate(
            &mut store,
            &project,
            &candidate_id,
            RejectionReason::TooNoisy,
            None,
            None,
        )
        .expect("reject should succeed");
    }

    // Default list (no params → pending filter) → 0 results
    let default_result = server
        .vestige_list_candidates(Parameters(ListCandidatesParams {
            status: None,
            r#type: None,
            limit: 50,
            include_rejected: false,
        }))
        .await
        .expect("list default should succeed");
    let default_env = envelope(&default_result);
    let default_candidates = default_env["candidates"].as_array().unwrap();
    assert_eq!(
        default_candidates.len(),
        0,
        "default list must exclude rejected, got: {default_candidates:?}"
    );

    // list {include_rejected: true} → 1 result
    let inclusive_result = server
        .vestige_list_candidates(Parameters(ListCandidatesParams {
            status: None,
            r#type: None,
            limit: 50,
            include_rejected: true,
        }))
        .await
        .expect("list with include_rejected should succeed");
    let inclusive_env = envelope(&inclusive_result);
    let inclusive_candidates = inclusive_env["candidates"].as_array().unwrap();
    assert_eq!(
        inclusive_candidates.len(),
        1,
        "include_rejected=true must surface rejected candidate, got: {inclusive_candidates:?}"
    );
}

#[tokio::test]
async fn approval_tool_absent() {
    // PRD §10.4 explicit non-goal: no vestige_approve_candidate or
    // vestige_reject_candidate tools on the MCP surface.
    let (_tmp, server, _project) = make_server("cand-tool-absent");

    assert!(
        server.get_tool("vestige_approve_candidate").is_none(),
        "vestige_approve_candidate must NOT be registered (PRD §10.4)"
    );
    assert!(
        server.get_tool("vestige_reject_candidate").is_none(),
        "vestige_reject_candidate must NOT be registered (PRD §10.4)"
    );

    // Sanity-check that the three candidate tools that ARE registered appear.
    assert!(
        server.get_tool("vestige_propose_candidate").is_some(),
        "vestige_propose_candidate must be registered"
    );
    assert!(
        server.get_tool("vestige_list_candidates").is_some(),
        "vestige_list_candidates must be registered"
    );
    assert!(
        server.get_tool("vestige_get_candidate").is_some(),
        "vestige_get_candidate must be registered"
    );
}

// SKIP: propose_disabled_in_config_errors (T10)
//
// The per-test config-override pattern requires either (a) mutating the
// `VestigeConfig` that `VestigeServer::new` receives, or (b) rebuilding the
// server with a modified config between the disable-step and the call.
//
// The `make_server` helper builds the `VestigeConfig` via `build_init_config`
// which always sets `mcp.allow_propose_candidate = true` (the default). To
// wire `allow_propose_candidate = false` we need to reach into the `Inner`
// struct, which is behind `Arc<Mutex<Inner>>` with `pub(crate)` visibility.
//
// The gating logic is already unit-tested in
// `crates/vestige-mcp/src/tools/propose_candidate.rs` (the `CANDIDATE_DISABLED`
// branch). Deferred to V0.3 when per-test config override is added to the
// harness.
