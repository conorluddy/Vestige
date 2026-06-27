//! V0.5.4 `vestige scan` smoke: one-shot session-log ingestion CLI.
//!
//! Drives a Claude Code transcript fixture (via the `VESTIGE_CLAUDE_ROOT` test seam, with a
//! dash-encoded directory mapping to the repo's cwd) through `vestige scan` using the
//! deterministic `fake` extraction provider, and asserts:
//! - `--dry-run` proposes nothing and leaves the inbox empty,
//! - a real scan files candidates that show up in `vestige inbox`,
//! - a second scan is idempotent (cursor advanced, nothing new).

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
    claude_root: PathBuf,
    codex_root: PathBuf,
}

fn fresh_repo() -> Repo {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let claude_root = tmp.path().join("claude");
    let codex_root = tmp.path().join("codex");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&claude_root).unwrap();
    std::fs::create_dir_all(&codex_root).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    Repo {
        _tmp: tmp,
        repo,
        home,
        claude_root,
        codex_root,
    }
}

fn vestige(repo: &Repo, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(&repo.repo)
        .env("HOME", &repo.home)
        .env("VESTIGE_LOG", "warn")
        .env("VESTIGE_CLAUDE_ROOT", &repo.claude_root)
        .env("VESTIGE_CODEX_ROOT", &repo.codex_root)
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

fn parse_json(out: &std::process::Output, ctx: &str) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{ctx} not JSON: {e}\n{stdout}"))
}

fn init(repo: &Repo) {
    let out = vestige(
        repo,
        &["init", "--name", "scan-smoke", "--no-install-skills"],
    );
    assert_ok(&out, "init");
}

/// Write a Claude Code transcript whose dash-encoded project dir maps to `repo`'s cwd, so the
/// adapter resolves it to this project. The repo's real (canonicalised) path becomes the
/// directory name with path separators replaced by dashes.
fn seed_claude_session(repo: &Repo) {
    let cwd = std::fs::canonicalize(&repo.repo).unwrap();
    // "/tmp/foo/bar" -> "-tmp-foo-bar"
    let encoded = cwd.to_string_lossy().replace('/', "-");
    let project_dir = repo.claude_root.join(encoded);
    std::fs::create_dir_all(&project_dir).unwrap();

    // Minimal Claude Code .jsonl: one line per turn, nested message shape.
    let session = "\
{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"We decided to use SQLite as the canonical store for durability.\"}]}}
{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Understood, recording that decision.\"}]}}
";
    std::fs::write(project_dir.join("session-abc.jsonl"), session).unwrap();
}

#[test]
fn scan_dry_run_then_real_then_idempotent() {
    let repo = fresh_repo();
    init(&repo);
    seed_claude_session(&repo);

    // 1. Dry-run proposes nothing and writes nothing.
    let out = vestige(
        &repo,
        &["scan", "--provider", "fake", "--dry-run", "--json"],
    );
    assert_ok(&out, "scan --dry-run");
    let v = parse_json(&out, "scan --dry-run");
    assert_eq!(v["dry_run"], Value::Bool(true));
    assert_eq!(v["candidates_proposed"], Value::from(0));
    assert!(
        v["sessions_scanned"].as_u64().unwrap() >= 1,
        "dry-run must still discover the seeded session"
    );

    // Inbox is still empty after a dry-run.
    let inbox = vestige(&repo, &["inbox", "--json"]);
    assert_ok(&inbox, "inbox after dry-run");
    let inbox_v = parse_json(&inbox, "inbox after dry-run");
    let count_after_dry = inbox_v["candidates"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(count_after_dry, 0, "dry-run must not write candidates");

    // 2. Real scan files candidates.
    let out = vestige(&repo, &["scan", "--provider", "fake", "--json"]);
    assert_ok(&out, "scan");
    let v = parse_json(&out, "scan");
    assert_eq!(v["dry_run"], Value::Bool(false));
    let proposed = v["candidates_proposed"].as_u64().unwrap();
    assert!(
        proposed >= 1,
        "real scan must propose at least one candidate"
    );

    // The candidates show up in the inbox.
    let inbox = vestige(&repo, &["inbox", "--json"]);
    assert_ok(&inbox, "inbox after scan");
    let inbox_v = parse_json(&inbox, "inbox after scan");
    let count_after_scan = inbox_v["candidates"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(count_after_scan >= 1, "scan must create inbox candidates");

    // 3. A second scan is idempotent — the cursor advanced, so nothing new.
    let out = vestige(&repo, &["scan", "--provider", "fake", "--json"]);
    assert_ok(&out, "scan again");
    let v = parse_json(&out, "scan again");
    assert_eq!(
        v["candidates_proposed"],
        Value::from(0),
        "second scan must propose nothing new"
    );
}
