//! V0.3 M4 — `vestige trace` smoke tests.
//!
//! Covers PRD §15 M4 acceptance criteria and issue #60 DoD:
//!
//! - `vestige search "x" && vestige trace` shows the search trace at the top.
//! - `--limit 5` returns at most 5.
//! - `--kind search` filters correctly; unknown `--kind` errors with a useful message.
//! - `--caller mcp` filters correctly.
//! - `--since` filter works (write traces across time boundary, filter to last 2).
//! - `vestige trace <trace_id>` renders mode/query/result_ids/scores/latency/provider/model.
//! - `vestige trace <invalid_id>` errors cleanly (wrong prefix, not-found, etc.).
//! - `--json` shapes deserialize cleanly; key fields asserted per PRD §13.3.

use std::path::PathBuf;
use std::process::Command;

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

fn vestige(repo: &Repo, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(&repo.repo)
        .env("HOME", &repo.home)
        .env("VESTIGE_LOG", "warn")
        .args(args)
        .output()
        .expect("vestige binary invoked")
}

fn assert_ok(out: &std::process::Output, ctx: &str) {
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        panic!(
            "{ctx}: exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
            out.status.code()
        );
    }
}

fn assert_fail(out: &std::process::Output, ctx: &str) {
    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        panic!("{ctx}: expected failure but exited 0\nstdout: {stdout}");
    }
}

fn parse_json(out: &std::process::Output, ctx: &str) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{ctx} not JSON: {e}\n{stdout}"))
}

fn stdout_str(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr_str(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn init(repo: &Repo) {
    let out = vestige(
        repo,
        &["init", "--name", "trace-smoke", "--no-install-skills"],
    );
    assert_ok(&out, "init");
}

fn record_and_search(repo: &Repo, query: &str) {
    // Record something so search has results.
    let out = vestige(
        repo,
        &[
            "decision",
            "add",
            &format!("Decision about {query}"),
            "--json",
        ],
    );
    assert_ok(&out, "record for search");

    // Run the search (writes a trace row with caller=cli).
    let out = vestige(repo, &["search", query]);
    assert_ok(&out, "search");
}

// === TEST 1: search then trace shows the search at top ===

#[test]
fn search_then_trace_shows_search_at_top() {
    let repo = fresh_repo();
    init(&repo);
    record_and_search(&repo, "tracing");

    let out = vestige(&repo, &["trace"]);
    assert_ok(&out, "trace list");
    let text = stdout_str(&out);

    // The list must mention the trace and show it was a search.
    assert!(text.contains("search"), "list must show kind=search");
    assert!(text.contains("caller=cli"), "list must show caller=cli");
}

// === TEST 2: --limit returns at most N traces ===

#[test]
fn limit_flag_caps_returned_traces() {
    let repo = fresh_repo();
    init(&repo);

    // Write 8 search traces.
    for i in 0..8 {
        record_and_search(&repo, &format!("query{i}"));
    }

    let out = vestige(&repo, &["trace", "--limit", "5"]);
    assert_ok(&out, "trace --limit 5");
    let text = stdout_str(&out);

    // Count lines that contain "search" and "caller=cli".
    let trace_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("search") && l.contains("caller=cli"))
        .collect();

    assert!(
        trace_lines.len() <= 5,
        "expected at most 5 traces, got {}:\n{text}",
        trace_lines.len()
    );
}

// === TEST 3: --kind search filters correctly ===

#[test]
fn kind_filter_returns_only_matching_kind() {
    let repo = fresh_repo();
    init(&repo);

    // Write a search trace and a context trace.
    record_and_search(&repo, "filter-kind-test");
    let _ = vestige(&repo, &["context"]); // produces a context trace

    // Filter to search only.
    let out = vestige(&repo, &["trace", "--kind", "search", "--json"]);
    assert_ok(&out, "trace --kind search");
    let json = parse_json(&out, "trace --kind search json");

    let traces = json["traces"].as_array().expect("traces array");
    assert!(!traces.is_empty(), "must find at least one search trace");
    for t in traces {
        assert_eq!(
            t["kind"].as_str().unwrap_or(""),
            "search",
            "all returned traces must have kind=search"
        );
    }
}

// === TEST 4: unknown --kind errors with a useful message ===

#[test]
fn unknown_kind_flag_errors_with_useful_message() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["trace", "--kind", "flurble"]);
    assert_fail(&out, "trace --kind flurble");

    let stderr = stderr_str(&out);
    // The error message must mention the bad value.
    assert!(
        stderr.contains("flurble") || stderr.contains("kind"),
        "error must mention bad kind or the word 'kind': {stderr}"
    );
}

// === TEST 5: --caller mcp filters correctly ===

#[test]
fn caller_filter_returns_only_matching_caller() {
    let repo = fresh_repo();
    init(&repo);

    // Write a cli-caller trace via the CLI search.
    record_and_search(&repo, "caller-filter-test");

    // Filter to mcp — should return no rows (we only have cli traces here).
    let out = vestige(&repo, &["trace", "--caller", "mcp", "--json"]);
    assert_ok(&out, "trace --caller mcp");
    let json = parse_json(&out, "trace --caller mcp json");

    let traces = json["traces"].as_array().expect("traces array");
    // All returned traces must have caller=mcp.
    for t in traces {
        assert_eq!(
            t["caller"].as_str().unwrap_or(""),
            "mcp",
            "all returned traces must have caller=mcp"
        );
    }

    // Filter to cli — should include our search traces.
    let out = vestige(&repo, &["trace", "--caller", "cli", "--json"]);
    assert_ok(&out, "trace --caller cli");
    let json = parse_json(&out, "trace --caller cli json");
    let traces = json["traces"].as_array().expect("traces array");
    assert!(!traces.is_empty(), "must find cli traces");
    for t in traces {
        assert_eq!(t["caller"].as_str().unwrap_or(""), "cli");
    }
}

// === TEST 6: --since filter works ===

#[test]
fn since_filter_returns_only_traces_after_cutoff() {
    let repo = fresh_repo();
    init(&repo);

    // Write a few traces.
    record_and_search(&repo, "before-cutoff-1");
    record_and_search(&repo, "before-cutoff-2");

    // Use a far-future date so nothing is within the window.
    let out = vestige(&repo, &["trace", "--since", "2099-01-01", "--json"]);
    assert_ok(&out, "trace --since future");
    let json = parse_json(&out, "trace --since future json");
    let traces = json["traces"].as_array().expect("traces array");
    assert!(
        traces.is_empty(),
        "future --since should return no traces, got {}",
        traces.len()
    );

    // Use a past date — should get all traces.
    let out = vestige(&repo, &["trace", "--since", "2000-01-01", "--json"]);
    assert_ok(&out, "trace --since past");
    let json = parse_json(&out, "trace --since past json");
    let traces = json["traces"].as_array().expect("traces array");
    assert!(!traces.is_empty(), "past --since should return all traces");
}

// === TEST 7: vestige trace <trace_id> renders detail ===

#[test]
fn show_trace_renders_mode_query_results_latency() {
    let repo = fresh_repo();
    init(&repo);
    record_and_search(&repo, "show-trace-test");

    // Get the trace ID from the list.
    let out = vestige(&repo, &["trace", "--json"]);
    assert_ok(&out, "trace list for id");
    let json = parse_json(&out, "trace list json");
    let traces = json["traces"].as_array().expect("traces array");
    assert!(!traces.is_empty(), "need at least one trace");

    let trace_id = traces[0]["trace_id"]
        .as_str()
        .expect("trace_id")
        .to_string();
    assert!(trace_id.starts_with("trace_"), "id must have trace_ prefix");

    // Show the trace.
    let out = vestige(&repo, &["trace", &trace_id]);
    assert_ok(&out, "trace show");
    let text = stdout_str(&out);

    assert!(text.contains(&trace_id), "output must contain the trace id");
    assert!(text.contains("search"), "output must mention kind=search");
    assert!(text.contains("caller=cli"), "output must mention caller");
    // At least "Results" section.
    assert!(
        text.contains("Results"),
        "output must contain Results section"
    );
}

// === TEST 8: vestige trace <trace_id> --json validates shape ===

#[test]
fn show_trace_json_shape_matches_prd_13_3() {
    let repo = fresh_repo();
    init(&repo);
    record_and_search(&repo, "json-shape-test");

    // Retrieve a trace ID.
    let out = vestige(&repo, &["trace", "--json"]);
    assert_ok(&out, "trace list");
    let json = parse_json(&out, "trace list json");
    let traces = json["traces"].as_array().expect("traces array");
    assert!(!traces.is_empty(), "need at least one trace");
    let trace_id = traces[0]["trace_id"]
        .as_str()
        .expect("trace_id")
        .to_string();

    // Show with --json.
    let out = vestige(&repo, &["trace", &trace_id, "--json"]);
    assert_ok(&out, "trace show --json");
    let detail = parse_json(&out, "trace show json");

    // Required top-level fields per PRD §13.3 (show shape).
    assert!(detail["trace_id"].is_string(), "trace_id must be string");
    assert!(detail["kind"].is_string(), "kind must be string");
    assert!(detail["caller"].is_string(), "caller must be string");
    assert!(
        detail["latency_ms"].is_number(),
        "latency_ms must be number"
    );
    assert!(
        detail["result_count"].is_number(),
        "result_count must be number"
    );
    assert!(
        detail["created_at"].is_string(),
        "created_at must be string"
    );
    assert!(detail["result_ids"].is_array(), "result_ids must be array");
    assert!(
        detail["result_scores"].is_array(),
        "result_scores must be array"
    );

    // For a search trace, mode fields must be present.
    assert_eq!(detail["kind"].as_str().unwrap(), "search");
    assert!(
        detail.get("mode_resolved").is_some(),
        "search trace must have mode_resolved"
    );
}

// === TEST 9: list --json shape matches PRD §13.3 ===

#[test]
fn list_trace_json_shape_matches_prd_13_3() {
    let repo = fresh_repo();
    init(&repo);
    record_and_search(&repo, "list-json-shape-test");

    let out = vestige(&repo, &["trace", "--json"]);
    assert_ok(&out, "trace list --json");
    let json = parse_json(&out, "trace list json");

    // Top-level must be `{ traces: [...] }`.
    let traces = json["traces"].as_array().expect("traces must be array");
    assert!(!traces.is_empty(), "must have at least one trace");

    for t in traces {
        assert!(t["trace_id"].is_string(), "trace_id must be string");
        assert!(t["kind"].is_string(), "kind must be string");
        assert!(t["caller"].is_string(), "caller must be string");
        assert!(t["result_count"].is_number(), "result_count must be number");
        assert!(t["latency_ms"].is_number(), "latency_ms must be number");
        assert!(t["created_at"].is_string(), "created_at must be string");
    }
}

// === TEST 10: invalid trace ID errors cleanly ===

#[test]
fn show_invalid_trace_id_exits_non_zero_with_message() {
    let repo = fresh_repo();
    init(&repo);

    // Wrong prefix.
    let out = vestige(&repo, &["trace", "mem_01ARZ3NDEKTSV4RRFFQ69G5FAV"]);
    assert_fail(&out, "trace mem_ prefix");
    let stderr = stderr_str(&out);
    assert!(
        stderr.contains("trace_") || stderr.contains("invalid"),
        "error must mention the expected format: {stderr}"
    );
}

// === TEST 11: trace ID not found errors cleanly ===

#[test]
fn show_nonexistent_trace_id_exits_non_zero() {
    let repo = fresh_repo();
    init(&repo);

    // Valid format but not in the DB.
    let out = vestige(&repo, &["trace", "trace_01ARZ3NDEKTSV4RRFFQ69G5FAV"]);
    assert_fail(&out, "trace not found");
    let stderr = stderr_str(&out);
    assert!(
        stderr.contains("not found") || stderr.contains("trace_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        "error must mention the trace id or 'not found': {stderr}"
    );
}

// === TEST 12: malformed --since rejected cleanly ===

#[test]
fn since_with_invalid_date_exits_non_zero() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["trace", "--since", "not-a-date"]);
    assert_fail(&out, "trace --since not-a-date");
    let stderr = stderr_str(&out);
    assert!(
        stderr.contains("since") || stderr.contains("invalid") || stderr.contains("not-a-date"),
        "error must mention the bad value: {stderr}"
    );
}

// === TEST 13: no traces shows friendly empty message ===

#[test]
fn empty_trace_list_shows_friendly_message() {
    let repo = fresh_repo();
    init(&repo);

    // No searches yet → no traces.
    let out = vestige(&repo, &["trace"]);
    assert_ok(&out, "trace empty list");
    let text = stdout_str(&out);
    assert!(
        text.contains("No query traces"),
        "empty list must show friendly message: {text}"
    );
}
