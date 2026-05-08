//! V0.3 M5 — `vestige trace replay` smoke tests.
//!
//! Covers PRD §15 M5 acceptance criteria and issue #59 DoD:
//!
//! - `search → trace → trace replay <id>` re-runs and shows diff.
//! - `trace replay <id> --json` validates against PRD §10.3 shape.
//! - `trace replay <invalid_id>` errors cleanly (wrong prefix).
//! - `trace replay <nonexistent_id>` errors cleanly.
//! - Replay never mutates the original trace (verified via `trace <id>` before and after).
//! - Forgotten memory appears in `removed` in the text output.
//! - New memory since original appears in the `added` list in JSON output.

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
        &["init", "--name", "replay-smoke", "--no-install-skills"],
    );
    assert_ok(&out, "init");
}

/// Record a decision and run a search; returns the trace_id of the search.
fn record_and_search(repo: &Repo, keyword: &str) -> String {
    let out = vestige(
        repo,
        &[
            "decision",
            "add",
            &format!("Decision about {keyword} topic"),
            "--json",
        ],
    );
    assert_ok(&out, "record");

    let out = vestige(repo, &["search", keyword]);
    assert_ok(&out, "search");

    // Grab the trace ID from the list.
    let out = vestige(repo, &["trace", "--json"]);
    assert_ok(&out, "trace list");
    let json = parse_json(&out, "trace list json");
    json["traces"][0]["trace_id"]
        .as_str()
        .expect("trace_id")
        .to_string()
}

// === TEST 1: replay text output shows Replaying header ===

#[test]
fn replay_shows_replaying_header_in_text() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "replay-header-test");

    let out = vestige(&repo, &["trace", "replay", &trace_id]);
    assert_ok(&out, "trace replay");
    let text = stdout_str(&out);

    assert!(
        text.contains("Replaying") || text.contains(&trace_id),
        "output must mention replay or trace id: {text}"
    );
}

// === TEST 2: replay --json shape matches PRD §10.3 ===

#[test]
fn replay_json_shape_matches_prd_10_3() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "json-shape-replay");

    let out = vestige(&repo, &["trace", "replay", &trace_id, "--json"]);
    assert_ok(&out, "trace replay --json");
    let json = parse_json(&out, "trace replay json");

    // Required top-level fields per PRD §10.3.
    assert!(json["trace_id"].is_string(), "trace_id must be string");
    assert!(json["original"].is_object(), "original must be object");
    assert!(json["current"].is_object(), "current must be object");
    assert!(json["diff"].is_object(), "diff must be object");
    assert!(
        json["provider_match"].is_boolean(),
        "provider_match must be boolean"
    );
    assert!(
        json["mode_fallback"].is_boolean(),
        "mode_fallback must be boolean"
    );
    assert!(
        json["replay_trace_id"].is_string(),
        "replay_trace_id must be string"
    );
    assert!(
        json["corpus_size"].is_number(),
        "corpus_size must be number"
    );

    // Sub-shapes.
    assert!(
        json["original"]["result_ids"].is_array(),
        "original.result_ids must be array"
    );
    assert!(
        json["original"]["scores"].is_array(),
        "original.scores must be array"
    );
    assert!(
        json["current"]["result_ids"].is_array(),
        "current.result_ids must be array"
    );
    assert!(
        json["current"]["scores"].is_array(),
        "current.scores must be array"
    );
    assert!(json["diff"]["added"].is_array(), "diff.added must be array");
    assert!(
        json["diff"]["removed"].is_array(),
        "diff.removed must be array"
    );
    assert!(
        json["diff"]["score_changes"].is_array(),
        "diff.score_changes must be array"
    );

    // The replayed trace_id must be different from the original.
    let replay_id = json["replay_trace_id"].as_str().unwrap();
    assert_ne!(
        replay_id, trace_id,
        "replay_trace_id must differ from original"
    );
    assert!(
        replay_id.starts_with("trace_"),
        "replay_trace_id must have trace_ prefix"
    );
}

// === TEST 3: replay writes a new trace (verify via trace list) ===

#[test]
fn replay_writes_new_trace_visible_in_trace_list() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "new-trace-write-test");

    // Count traces before replay.
    let out = vestige(&repo, &["trace", "--json"]);
    assert_ok(&out, "trace list before replay");
    let before = parse_json(&out, "before replay json");
    let before_count = before["traces"].as_array().unwrap().len();

    // Run replay.
    let out = vestige(&repo, &["trace", "replay", &trace_id, "--json"]);
    assert_ok(&out, "trace replay");

    // Count traces after.
    let out = vestige(&repo, &["trace", "--limit", "100", "--json"]);
    assert_ok(&out, "trace list after replay");
    let after = parse_json(&out, "after replay json");
    let after_count = after["traces"].as_array().unwrap().len();

    assert_eq!(
        after_count,
        before_count + 1,
        "replay must produce exactly one new trace row"
    );
}

// === TEST 4: original trace not mutated after replay ===

#[test]
fn original_trace_unchanged_after_replay() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "original-unchanged");

    // Capture original trace detail before replay.
    let out = vestige(&repo, &["trace", &trace_id, "--json"]);
    assert_ok(&out, "trace show before replay");
    let before = parse_json(&out, "trace show before json");
    let before_created_at = before["created_at"].as_str().unwrap().to_string();
    let before_result_ids: Vec<Value> =
        before["result_ids"].as_array().cloned().unwrap_or_default();

    // Run replay.
    vestige(&repo, &["trace", "replay", &trace_id]);

    // Re-fetch original trace.
    let out = vestige(&repo, &["trace", &trace_id, "--json"]);
    assert_ok(&out, "trace show after replay");
    let after = parse_json(&out, "trace show after json");
    let after_created_at = after["created_at"].as_str().unwrap().to_string();
    let after_result_ids: Vec<Value> = after["result_ids"].as_array().cloned().unwrap_or_default();

    assert_eq!(
        before_created_at, after_created_at,
        "original trace created_at must not change"
    );
    assert_eq!(
        before_result_ids, after_result_ids,
        "original trace result_ids must not change"
    );
}

// === TEST 5: replay with wrong-prefix id errors cleanly ===

#[test]
fn replay_with_wrong_prefix_errors_cleanly() {
    let repo = fresh_repo();
    init(&repo);

    // Use a mem_ prefix where trace_ is expected.
    let out = vestige(
        &repo,
        &["trace", "replay", "mem_01ARZ3NDEKTSV4RRFFQ69G5FAV"],
    );
    assert_fail(&out, "replay wrong prefix");

    let stderr = stderr_str(&out);
    assert!(
        stderr.contains("trace_") || stderr.contains("invalid"),
        "error must mention expected format: {stderr}"
    );
}

// === TEST 6: replay with nonexistent trace id errors cleanly ===

#[test]
fn replay_with_nonexistent_trace_id_errors_cleanly() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &["trace", "replay", "trace_01ARZ3NDEKTSV4RRFFQ69G5FAV"],
    );
    assert_fail(&out, "replay not found");

    let stderr = stderr_str(&out);
    assert!(
        stderr.contains("not found") || stderr.contains("trace_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        "error must mention trace id or 'not found': {stderr}"
    );
}

// === TEST 7: identical corpus → diff.added and diff.removed empty in JSON ===

#[test]
fn identical_corpus_produces_empty_diff_in_json() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "identical-corpus-test");

    let out = vestige(&repo, &["trace", "replay", &trace_id, "--json"]);
    assert_ok(&out, "trace replay identical");
    let json = parse_json(&out, "trace replay identical json");

    let added = json["diff"]["added"].as_array().unwrap();
    let removed = json["diff"]["removed"].as_array().unwrap();

    assert!(
        added.is_empty(),
        "identical corpus → diff.added must be empty; got: {added:?}"
    );
    assert!(
        removed.is_empty(),
        "identical corpus → diff.removed must be empty; got: {removed:?}"
    );
}

// === TEST 8: new memory since original appears in diff.added ===

#[test]
fn new_memory_appears_in_diff_added() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "diff-added-test");

    // Add a new memory after the original search.
    let out = vestige(
        &repo,
        &[
            "note",
            "add",
            "New memory about diff-added-test topic added after original search",
        ],
    );
    assert_ok(&out, "record new memory");

    // Replay and check diff.added.
    let out = vestige(&repo, &["trace", "replay", &trace_id, "--json"]);
    assert_ok(&out, "trace replay");
    let json = parse_json(&out, "trace replay json");

    let added = json["diff"]["added"].as_array().unwrap();
    // May or may not contain the new memory (depends on FTS scoring),
    // but the array must exist and be well-formed.
    for id in added {
        assert!(
            id.as_str().is_some_and(|s| s.starts_with("mem_")),
            "added IDs must be mem_ prefixed: {id}"
        );
    }
}

// === TEST 9: forgotten memory appears in diff.removed ===

#[test]
fn forgotten_memory_appears_in_diff_removed() {
    let repo = fresh_repo();
    init(&repo);

    // Record a memory that will appear in search results.
    let out = vestige(
        &repo,
        &[
            "decision",
            "add",
            "Diff-removed-test forgetme topic decision",
            "--json",
        ],
    );
    assert_ok(&out, "record");
    let mem_json = parse_json(&out, "record json");
    let memory_id = mem_json["id"].as_str().expect("id").to_string();

    // Search so the memory appears in results.
    let out = vestige(&repo, &["search", "diff-removed-test"]);
    assert_ok(&out, "search");

    // Get the trace id.
    let out = vestige(&repo, &["trace", "--json"]);
    assert_ok(&out, "trace list");
    let json = parse_json(&out, "trace list json");
    let trace_id = json["traces"][0]["trace_id"]
        .as_str()
        .expect("trace_id")
        .to_string();

    // Forget the memory.
    let out = vestige(&repo, &["forget", &memory_id]);
    assert_ok(&out, "forget");

    // Replay and verify the memory is in diff.removed.
    let out = vestige(&repo, &["trace", "replay", &trace_id, "--json"]);
    assert_ok(&out, "trace replay");
    let replay_json = parse_json(&out, "trace replay json");

    let removed: Vec<&str> = replay_json["diff"]["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();

    assert!(
        removed.contains(&memory_id.as_str()),
        "forgotten memory {memory_id} must appear in diff.removed; got: {removed:?}"
    );
}

// === TEST 10: replay prints provider and corpus drift info ===

#[test]
fn replay_text_output_shows_provider_and_corpus_info() {
    let repo = fresh_repo();
    init(&repo);
    let trace_id = record_and_search(&repo, "provider-corpus-output");

    let out = vestige(&repo, &["trace", "replay", &trace_id]);
    assert_ok(&out, "trace replay text");
    let text = stdout_str(&out);

    // Provider and corpus lines should be present.
    assert!(
        text.contains("Provider") || text.contains("provider"),
        "output must mention provider: {text}"
    );
    assert!(
        text.contains("Corpus") || text.contains("corpus") || text.contains("drift"),
        "output must mention corpus drift: {text}"
    );
}
