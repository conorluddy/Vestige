//! M8 smoke: MCP `vestige_search` mode extensions (PR6).
//!
//! Tests all three search modes via the in-process JSON-RPC harness from m5.
//! Shared helpers are duplicated here (rather than extracted) — AHA over DRY.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

struct Repo {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn fresh_repo() -> Repo {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    Repo {
        _tmp: tmp,
        repo,
        home,
    }
}

fn run_cli(repo: &Repo, args: &[&str]) {
    let out = Command::new(binary())
        .current_dir(&repo.repo)
        .env("HOME", &repo.home)
        .env("VESTIGE_LOG", "warn")
        .args(args)
        .output()
        .expect("vestige binary");
    if !out.status.success() {
        panic!(
            "{:?} failed: {}\n{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    fn spawn(repo: &Repo, extra_args: &[&str]) -> Self {
        let mut args = vec!["mcp"];
        args.extend_from_slice(extra_args);
        let mut child = Command::new(binary())
            .current_dir(&repo.repo)
            .env("HOME", &repo.home)
            .env("VESTIGE_LOG", "warn")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn vestige mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&req).unwrap();
        self.stdin.write_all(body.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&req).unwrap();
        self.stdin.write_all(body.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read_response(&mut self, id: i64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                panic!("timed out waiting for response id={id}");
            }
            let mut line = String::new();
            match self.stdout.read_line(&mut line) {
                Ok(0) => {
                    let mut stderr = String::new();
                    if let Some(mut e) = self.child.stderr.take() {
                        let _ = e.read_to_string(&mut stderr);
                    }
                    panic!("server closed stdout. stderr:\n{stderr}");
                }
                Ok(_) => {}
                Err(e) => panic!("read_line failed: {e}"),
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(trimmed)
                .unwrap_or_else(|e| panic!("non-JSON line: {e}: {trimmed}"));
            if v.get("id").and_then(|x| x.as_i64()) == Some(id) {
                return v;
            }
        }
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize(client: &mut McpClient) {
    let id = client.send(
        "initialize",
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "m8-smoke", "version": "0.0.0" }
        }),
    );
    let resp = client.read_response(id, Duration::from_secs(5));
    assert!(resp.get("result").is_some(), "initialize failed: {resp}");
    client.notify("notifications/initialized", serde_json::json!({}));
}

fn call_tool(client: &mut McpClient, name: &str, args: Value) -> Value {
    let id = client.send(
        "tools/call",
        serde_json::json!({ "name": name, "arguments": args }),
    );
    client.read_response(id, Duration::from_secs(5))
}

fn extract_text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("expected text content in {resp}"))
        .to_string()
}

/// Extract the search envelope from a tool call response.
fn extract_envelope(resp: &Value) -> Value {
    let text = extract_text(resp);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("failed to parse search envelope: {e}: {text}"))
}

// ========================================
// === TESTS ===
// ========================================

/// Calling vestige_search without `mode` must return mode="lexical" (backwards
/// compat). V0 returned a flat array; PR6 wraps it in `{mode, results, warnings}`.
/// This is a breaking change — agents must update to read `envelope.results`.
#[test]
fn search_default_mode_is_lexical_backwards_compat() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "ModeTest"]);
    run_cli(
        &repo,
        &[
            "remember",
            "Use SQLite as the canonical store for memories.",
        ],
    );

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "SQLite" }),
    );
    let envelope = extract_envelope(&resp);

    assert_eq!(
        envelope["mode"].as_str(),
        Some("lexical"),
        "default mode must be lexical: {envelope}"
    );
    assert!(
        envelope["results"].is_array(),
        "results must be an array: {envelope}"
    );
    assert!(
        envelope["warnings"].is_array(),
        "warnings must be an array: {envelope}"
    );
    assert!(
        !envelope["results"].as_array().unwrap().is_empty(),
        "search should find the memory: {envelope}"
    );

    client.shutdown();
}

/// Semantic mode against a project with no embeddings must return a structured
/// error with code "EMBEDDINGS_UNAVAILABLE".
#[test]
fn search_semantic_no_embeddings_returns_structured_error() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "SemanticTest"]);
    run_cli(&repo, &["remember", "No embeddings exist yet."]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "embeddings", "mode": "semantic" }),
    );

    // Must be an error response (isError true or top-level error field).
    let is_error = resp.get("error").is_some() || resp["result"]["isError"].as_bool() == Some(true);
    assert!(
        is_error,
        "semantic mode with no embeddings must return an error: {resp}"
    );

    let resp_str = resp.to_string();
    assert!(
        resp_str.contains("EMBEDDINGS_UNAVAILABLE"),
        "error code must be 'EMBEDDINGS_UNAVAILABLE': {resp}"
    );

    client.shutdown();
}

/// An unrecognised mode string must return a structured error with code "INVALID_MODE".
#[test]
fn search_invalid_mode_returns_structured_error() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "InvalidMode"]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "anything", "mode": "fuzzy" }),
    );

    let is_error = resp.get("error").is_some() || resp["result"]["isError"].as_bool() == Some(true);
    assert!(is_error, "unknown mode must return an error: {resp}");

    let resp_str = resp.to_string();
    assert!(
        resp_str.contains("INVALID_MODE"),
        "error code must be 'INVALID_MODE': {resp}"
    );

    client.shutdown();
}

/// Hybrid mode with no embeddings falls back to lexical and surfaces a warning.
/// Results must be non-empty (lexical fallback worked) and warnings non-empty.
#[test]
fn search_hybrid_falls_back_lexical_with_warning() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "HybridFallback"]);
    run_cli(&repo, &["remember", "Use SQLite as canonical store."]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "SQLite canonical", "mode": "hybrid" }),
    );
    let envelope = extract_envelope(&resp);

    assert_eq!(
        envelope["mode"].as_str(),
        Some("hybrid"),
        "mode must remain 'hybrid' even on fallback: {envelope}"
    );
    let warnings = envelope["warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty(),
        "hybrid fallback must include a warning: {envelope}"
    );
    let warning_text = warnings[0].as_str().unwrap_or("");
    assert!(
        warning_text.contains("lexical") || warning_text.contains("embedding"),
        "warning must mention lexical or embedding: {envelope}"
    );
    let results = envelope["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "lexical fallback must return results: {envelope}"
    );

    client.shutdown();
}

/// When embeddings exist, hybrid mode must return results with score_parts
/// populated: fts, vector, importance, type_boost, total.
#[test]
fn search_hybrid_with_embeddings_returns_score_parts() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "HybridWithEmbeddings"]);
    run_cli(
        &repo,
        &["remember", "SQLite is the canonical store for memories."],
    );

    // Embed with the fake provider via the CLI embed command.
    run_cli(&repo, &["embed", "--all", "--provider", "fake"]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "SQLite canonical", "mode": "hybrid" }),
    );
    let envelope = extract_envelope(&resp);

    assert_eq!(
        envelope["mode"].as_str(),
        Some("hybrid"),
        "mode must be hybrid: {envelope}"
    );
    let results = envelope["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "hybrid with embeddings must return results: {envelope}"
    );

    // Every result must have score_parts with the expected fields.
    for result in results {
        let parts = &result["score_parts"];
        assert!(
            !parts.is_null(),
            "hybrid result must have score_parts: {result}"
        );
        for field in &["fts", "vector", "importance", "type_boost", "total"] {
            assert!(
                parts[field].is_number(),
                "score_parts.{field} must be a number: {result}"
            );
        }
    }

    client.shutdown();
}

/// Read-only mode must still allow vestige_search (all modes are read-only ops).
/// Record tools must still be blocked.
#[test]
fn read_only_mode_still_allows_search() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "ReadOnlySearch"]);
    run_cli(
        &repo,
        &["remember", "Search test memory for readonly mode."],
    );

    let mut client = McpClient::spawn(&repo, &["--read-only"]);
    initialize(&mut client);

    // Search must succeed in read-only mode.
    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "readonly", "mode": "lexical" }),
    );
    let envelope = extract_envelope(&resp);
    assert_eq!(
        envelope["mode"].as_str(),
        Some("lexical"),
        "search must work in read-only mode: {envelope}"
    );

    // Record must still be blocked.
    let resp = call_tool(
        &mut client,
        "vestige_record_decision",
        serde_json::json!({ "decision": "Should not land." }),
    );
    let blocked = resp.get("error").is_some()
        || resp["result"]["isError"].as_bool() == Some(true)
        || resp.to_string().contains("READ_ONLY");
    assert!(blocked, "read-only must reject record_decision: {resp}");

    client.shutdown();
}

/// Regression test for the V0.1 silent-empty-results trap.
///
/// When `.vestige/config.toml` configures a different provider/dimensions than
/// what the project was actually embedded with, MCP semantic search must NOT
/// silently return an empty list — it must surface a structured error so the
/// agent knows to re-embed.
#[test]
fn semantic_mismatched_provider_returns_structured_error_not_empty() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "MismatchedProvider"]);
    run_cli(
        &repo,
        &["remember", "Configured provider differs from stored."],
    );

    // Embed with fake@64 (the default).
    run_cli(&repo, &["embed", "--all", "--provider", "fake"]);

    // Now switch the configured runtime to fake@128 — same provider name but
    // different dimensions. The dominant stored row is 64d; the runtime would
    // build a 128d query vector, which `nearest_neighbours` filters out by
    // dimensions and silently returned [] in V0.1 before this guard.
    let cfg_path = repo.repo.join(".vestige/config.toml");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[embeddings]\nprovider = \"fake\"\ndimensions = 128\n");
    std::fs::write(&cfg_path, cfg).unwrap();

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "configured", "mode": "semantic" }),
    );

    let is_error = resp.get("error").is_some() || resp["result"]["isError"].as_bool() == Some(true);
    assert!(
        is_error,
        "mismatch must return an error, not silent []: {resp}"
    );

    let resp_str = resp.to_string();
    assert!(
        resp_str.contains("EMBEDDINGS_UNAVAILABLE"),
        "error code must name the mismatch case: {resp}"
    );
    assert!(
        resp_str.contains("vestige embed --all"),
        "error message must direct the agent to re-embed: {resp}"
    );

    client.shutdown();
}

/// Hybrid mode under the same mismatch must degrade to lexical-with-warning
/// rather than error — hybrid's contract is "always return something".
#[test]
fn hybrid_mismatched_provider_falls_back_to_lexical_with_warning() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "MismatchedHybrid"]);
    run_cli(&repo, &["remember", "Hybrid graceful fallback path."]);
    run_cli(&repo, &["embed", "--all", "--provider", "fake"]);

    let cfg_path = repo.repo.join(".vestige/config.toml");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[embeddings]\nprovider = \"fake\"\ndimensions = 128\n");
    std::fs::write(&cfg_path, cfg).unwrap();

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "graceful fallback", "mode": "hybrid" }),
    );
    let envelope = extract_envelope(&resp);

    assert_eq!(
        envelope["mode"].as_str(),
        Some("hybrid"),
        "envelope mode must remain hybrid: {envelope}"
    );
    let warnings = envelope["warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty(),
        "mismatched hybrid must include a warning: {envelope}"
    );
    let warning_text = warnings[0].as_str().unwrap_or("");
    assert!(
        warning_text.contains("lexical"),
        "warning must mention lexical fallback: {envelope}"
    );
    let results = envelope["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "lexical fallback must still return results: {envelope}"
    );

    client.shutdown();
}

/// Positive: semantic search must work when the configured provider matches
/// what the project was embedded with.
#[test]
fn semantic_with_matching_configured_provider_returns_hits() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "MatchingProvider"]);
    run_cli(
        &repo,
        &["remember", "Memory body for matching provider test."],
    );

    // Configure fake explicitly, embed under that config.
    let cfg_path = repo.repo.join(".vestige/config.toml");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[embeddings]\nprovider = \"fake\"\n");
    std::fs::write(&cfg_path, cfg).unwrap();
    run_cli(&repo, &["embed", "--all"]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "matching provider", "mode": "semantic" }),
    );
    let envelope = extract_envelope(&resp);

    assert_eq!(envelope["mode"].as_str(), Some("semantic"));
    let results = envelope["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "matching configured provider must return hits: {envelope}"
    );

    client.shutdown();
}
