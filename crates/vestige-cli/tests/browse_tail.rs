//! CLI smoke test for the Tail tab in `vestige browse`.
//!
//! Validates three things:
//!
//! 1. `browse --help` lists `tail` as a valid `--tab` value.
//! 2. `browse --tab tail` exits with the documented TTY guard when stdin/stdout
//!    are not a terminal — same guard that protects all browse invocations.
//! 3. Memories and candidates seeded via the CLI appear in `list` and `list
//!    --type decision` output, confirming the data layer that the Tail tab
//!    queries is populated correctly in an isolated project.
//!
//! In-process `TestBackend` rendering of the Tail tab is covered by unit tests
//! inside `crates/vestige-cli/src/commands/browse/ui.rs` (the same module that
//! owns `draw`). External integration tests cannot reach those private items in
//! a binary crate.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

// === HELPERS ===

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

struct IsolatedProject {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn fresh_project() -> IsolatedProject {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    // A `.git` dir makes project-id resolution use the path hash rather than
    // walking up to find a parent git root — keeps tests hermetic.
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    IsolatedProject {
        _tmp: tmp,
        repo,
        home,
    }
}

fn vestige(project: &IsolatedProject, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(&project.repo)
        .env("HOME", &project.home)
        .env("VESTIGE_LOG", "warn")
        .args(args)
        .output()
        .expect("vestige binary invoked")
}

fn assert_ok(out: &std::process::Output, context: &str) {
    if !out.status.success() {
        panic!(
            "{context} failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn parse_json_array(out: &std::process::Output, context: &str) -> Vec<Value> {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    let value: Value = serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("{context} not JSON: {e}\n{stdout}"));
    value
        .as_array()
        .unwrap_or_else(|| panic!("{context} expected array"))
        .clone()
}

// === TESTS ===

/// `browse --help` must list `tail` as a valid value for `--tab`.
#[test]
fn browse_help_lists_tail_as_tab_option() {
    let out = Command::new(binary())
        .args(["browse", "--help"])
        .output()
        .expect("vestige binary invoked");
    assert!(out.status.success(), "browse --help should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("tail"),
        "--help output should list `tail` as a tab value: {stdout}"
    );
}

/// `browse --tab tail` without a TTY must fail with the documented guard
/// message, not silently corrupt terminal state.
#[test]
fn browse_tail_without_tty_exits_with_tty_guard() {
    let project = fresh_project();
    let out = vestige(&project, &["browse", "--tab", "tail"]);
    assert!(
        !out.status.success(),
        "browse --tab tail without a TTY should fail; got success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("TTY"),
        "stderr should mention TTY requirement: {stderr}"
    );
}

/// Seed memories and a candidate via the CLI; confirm the store contains the
/// expected rows — these are the rows the Tail tab's `reload` function queries.
/// This test is the data-layer half of the Tail tab smoke; the render half
/// lives in `src/commands/browse/ui.rs` where `draw` is reachable.
#[test]
fn tail_tab_data_layer_returns_seeded_memories_and_candidates() {
    let project = fresh_project();

    // Initialise the project.
    assert_ok(&vestige(&project, &["init", "--name", "TailSmoke"]), "init");

    // Seed a decision memory.
    let decision_out = vestige(
        &project,
        &[
            "decision",
            "add",
            "Use polling over push for the Tail tab.",
            "--rationale",
            "Keeps the store layer simple — no change-notification surface needed.",
            "--json",
        ],
    );
    assert_ok(&decision_out, "decision add");
    let decision: Value = serde_json::from_slice(&decision_out.stdout).unwrap();
    let decision_id = decision["id"].as_str().unwrap().to_string();
    assert!(
        decision_id.starts_with("mem_"),
        "decision id should carry mem_ prefix: {decision_id}"
    );

    // Seed a note memory.
    assert_ok(
        &vestige(
            &project,
            &["note", "add", "Tail tab polls every 60 s by default."],
        ),
        "note add",
    );

    // Seed a candidate via `candidate add`.
    let candidate_out = vestige(
        &project,
        &[
            "candidate",
            "add",
            "--type",
            "decision",
            "--body",
            "Auto-scroll pauses when the cursor leaves the top row.",
            "--json",
        ],
    );
    assert_ok(&candidate_out, "candidate add");
    // `candidate add --json` returns `{"candidate_id": "cand_…", …}`.
    let candidate_response: Value = serde_json::from_slice(&candidate_out.stdout).unwrap();
    let candidate_id = candidate_response["candidate_id"]
        .as_str()
        .unwrap_or_else(|| panic!("candidate_id missing in: {candidate_response}"))
        .to_string();
    assert!(
        candidate_id.starts_with("cand_"),
        "candidate id should carry cand_ prefix: {candidate_id}"
    );

    // Verify memories are visible via `list --json`.
    let list_out = vestige(&project, &["list", "--json"]);
    assert_ok(&list_out, "list");
    let memories = parse_json_array(&list_out, "list");
    // project_summary + decision + note = at least 2
    assert!(
        memories.len() >= 2,
        "expected at least 2 memories, got {}",
        memories.len()
    );
    let ids: Vec<&str> = memories.iter().filter_map(|m| m["id"].as_str()).collect();
    assert!(
        ids.contains(&decision_id.as_str()),
        "decision {decision_id} should appear in list: {ids:?}"
    );

    // Verify the candidate is in the inbox via `inbox --json`.
    // Response shape: `{"candidates": [{"id": "cand_…", …}]}`.
    let cands_out = vestige(&project, &["inbox", "--json"]);
    assert_ok(&cands_out, "inbox");
    let inbox_response: Value = serde_json::from_slice(&cands_out.stdout).unwrap();
    let candidates = inbox_response["candidates"]
        .as_array()
        .unwrap_or_else(|| panic!("inbox.candidates missing in: {inbox_response}"));
    assert_eq!(candidates.len(), 1, "expected exactly one candidate");
    assert_eq!(
        candidates[0]["id"].as_str().unwrap(),
        candidate_id,
        "candidate id should match"
    );
}
