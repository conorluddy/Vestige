//! M5 smoke: spawn `vestige mcp`, drive it with raw MCP JSON-RPC frames over
//! stdio, assert tools can be listed and called.
//!
//! Frames are LSP-style with Content-Length headers per the MCP spec.

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
        // rmcp's stdio transport uses newline-delimited JSON, not LSP framing.
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
            // Other messages (notifications, sampling, etc.) — ignore for V0.
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
            "clientInfo": { "name": "m5-smoke", "version": "0.0.0" }
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

#[test]
fn mcp_lists_six_tools() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "Mcp"]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    let id = client.send("tools/list", serde_json::json!({}));
    let resp = client.read_response(id, Duration::from_secs(5));
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "vestige_bootstrap",
        "vestige_search",
        "vestige_expand",
        "vestige_get_project_context",
        "vestige_record_observation",
        "vestige_record_decision",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    client.shutdown();
}

#[test]
fn mcp_record_search_expand_lifecycle() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "Mcp", "--summary", "MCP test."]);

    let mut client = McpClient::spawn(&repo, &[]);
    initialize(&mut client);

    // Record a decision via MCP.
    let resp = call_tool(
        &mut client,
        "vestige_record_decision",
        serde_json::json!({
            "decision": "MCP is a thin adapter over the engine.",
            "rationale": "Keeps storage and lifecycle in core.",
        }),
    );
    let card: Value = serde_json::from_str(&extract_text(&resp)).unwrap();
    let id = card["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("mem_"));
    assert_eq!(card["type"].as_str(), Some("decision"));

    // Search for it — result is now the { mode, results, warnings } envelope.
    let resp = call_tool(
        &mut client,
        "vestige_search",
        serde_json::json!({ "query": "MCP adapter", "limit": 5 }),
    );
    let envelope: Value = serde_json::from_str(&extract_text(&resp)).unwrap();
    assert_eq!(
        envelope["mode"].as_str(),
        Some("lexical"),
        "default mode must be lexical"
    );
    let arr = envelope["results"].as_array().unwrap();
    assert!(!arr.is_empty(), "search should hit MCP decision");
    assert_eq!(arr[0]["id"].as_str(), Some(id.as_str()));

    // Expand at full depth.
    let resp = call_tool(
        &mut client,
        "vestige_expand",
        serde_json::json!({ "memory_id": id, "depth": "full" }),
    );
    let detail: Value = serde_json::from_str(&extract_text(&resp)).unwrap();
    let content = detail["content"].as_str().unwrap();
    assert!(content.contains("MCP is a thin adapter"));
    assert!(content.contains("Rationale: Keeps storage"));

    // Project context.
    let resp = call_tool(
        &mut client,
        "vestige_get_project_context",
        serde_json::json!({ "budget_tokens": 800 }),
    );
    let pack: Value = serde_json::from_str(&extract_text(&resp)).unwrap();
    let text = pack["text"].as_str().unwrap();
    assert!(text.contains("Project: Mcp"));
    assert!(text.contains("MCP is a thin adapter"));
    assert!(text.contains("MCP test."));

    client.shutdown();
}

#[test]
fn read_only_blocks_record_tools() {
    let repo = fresh_repo();
    run_cli(&repo, &["init", "--name", "Ro"]);

    let mut client = McpClient::spawn(&repo, &["--read-only"]);
    initialize(&mut client);

    let resp = call_tool(
        &mut client,
        "vestige_record_decision",
        serde_json::json!({ "decision": "Should not land." }),
    );
    // Either an explicit error response or an isError result — accept both shapes.
    let blocked = resp.get("error").is_some()
        || resp["result"]["isError"].as_bool() == Some(true)
        || resp.to_string().contains("READ_ONLY");
    assert!(blocked, "read-only must reject record_decision: {resp}");

    client.shutdown();
}
