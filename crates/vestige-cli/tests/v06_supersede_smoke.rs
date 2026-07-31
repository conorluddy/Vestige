//! V0.6 — Supersede smoke tests (issue #131).
//!
//! Covers the issue's Definition of Done end to end through the CLI:
//!
//! - `remember A → remember B --supersedes A → search (A gone, B present) →
//!   why A (superseded_by=B) → why B (supersedes=A) → restore A (link
//!   cleared, A active, B untouched)` — the full lifecycle smoke.
//! - `--supersedes` reaches every typed capture command via the shared
//!   `capture::add` path (spot-checked with `decision`; `remember` covers
//!   the non-`CaptureAddArgs` path separately).
//! - `vestige approve <cand> --supersedes mem_X` supersedes in one call.
//! - Superseding a non-existent/already-deleted memory fails loudly rather
//!   than silently no-op-ing (CLI never hides the two-step window's failure).

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
        panic!(
            "{ctx}: exit {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn assert_fail(out: &std::process::Output, ctx: &str) {
    if out.status.success() {
        panic!(
            "{ctx}: expected failure but exited 0\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
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
        &["init", "--name", "supersede-smoke", "--no-install-skills"],
    );
    assert_ok(&out, "init");
}

// === TEST 1: full lifecycle smoke from the issue's DoD ===

#[test]
fn supersede_full_lifecycle_remember_search_why_restore() {
    let repo = fresh_repo();
    init(&repo);

    // remember A
    let out = vestige(
        &repo,
        &[
            "remember",
            "Ship the daemon as a LaunchAgent on macOS",
            "--json",
        ],
    );
    assert_ok(&out, "remember A");
    let mem_a = parse_json(&out, "remember A json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // remember B --supersedes A
    let out = vestige(
        &repo,
        &[
            "remember",
            "Ship the daemon as a systemd unit on macOS and Linux",
            "--supersedes",
            &mem_a,
            "--json",
        ],
    );
    assert_ok(&out, "remember B --supersedes A");
    let recorded_b = parse_json(&out, "remember B json");
    let mem_b = recorded_b["id"].as_str().unwrap().to_string();
    assert_eq!(
        recorded_b["supersedes"].as_str(),
        Some(mem_a.as_str()),
        "record output must echo the supersede link"
    );

    // search: A gone, B present.
    //
    // The query must match *both* memories for this assertion to mean
    // anything — otherwise "A is absent" passes trivially because A never
    // matched. FTS5 ANDs bare terms, so a query naming a token unique to one
    // memory (`LaunchAgent`, `systemd`) matches neither. "daemon" is in both.
    let out = vestige(&repo, &["search", "daemon", "--lexical", "--json"]);
    assert_ok(&out, "search after supersede");
    let json = parse_json(&out, "search json");
    let ids: Vec<&str> = json["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&mem_a.as_str()),
        "A must drop out of search; got {ids:?}"
    );
    assert!(
        ids.contains(&mem_b.as_str()),
        "B must appear in search; got {ids:?}"
    );

    // why A --json: superseded_by = B
    let out = vestige(&repo, &["why", &mem_a, "--json"]);
    assert_ok(&out, "why A");
    let why_a = parse_json(&out, "why A json");
    assert_eq!(why_a["status"].as_str(), Some("deleted"));
    assert_eq!(
        why_a["provenance"]["superseded_by"].as_str(),
        Some(mem_b.as_str()),
        "why A must report superseded_by = B"
    );

    // why B --json: supersedes = A
    let out = vestige(&repo, &["why", &mem_b, "--json"]);
    assert_ok(&out, "why B");
    let why_b = parse_json(&out, "why B json");
    assert_eq!(why_b["status"].as_str(), Some("active"));
    assert_eq!(
        why_b["provenance"]["supersedes"].as_str(),
        Some(mem_a.as_str()),
        "why B must report supersedes = A"
    );

    // restore A: link cleared, A active, B untouched
    let out = vestige(&repo, &["restore", &mem_a]);
    assert_ok(&out, "restore A");

    let out = vestige(&repo, &["why", &mem_a, "--json"]);
    assert_ok(&out, "why A after restore");
    let why_a_restored = parse_json(&out, "why A restored json");
    assert_eq!(why_a_restored["status"].as_str(), Some("active"));
    assert!(
        why_a_restored["provenance"]
            .get("superseded_by")
            .map(|v| v.is_null())
            .unwrap_or(true),
        "why A must not report superseded_by after restore; got {why_a_restored}"
    );

    let out = vestige(&repo, &["why", &mem_b, "--json"]);
    assert_ok(&out, "why B after A restored");
    let why_b_after = parse_json(&out, "why B after restore json");
    assert_eq!(
        why_b_after["status"].as_str(),
        Some("active"),
        "restoring A must not cascade-affect B"
    );
}

// === TEST 2: --supersedes reaches typed capture commands via the shared path ===

#[test]
fn decision_supersedes_via_shared_capture_path() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["decision", "add", "Use tokio 1.x", "--json"]);
    assert_ok(&out, "decision add A");
    let mem_a = parse_json(&out, "decision add A json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = vestige(
        &repo,
        &[
            "decision",
            "add",
            "Use tokio 1.x with the multi-thread runtime",
            "--supersedes",
            &mem_a,
            "--json",
        ],
    );
    assert_ok(&out, "decision add B --supersedes A");
    let json = parse_json(&out, "decision add B json");
    assert_eq!(json["supersedes"].as_str(), Some(mem_a.as_str()));

    let out = vestige(&repo, &["why", &mem_a, "--json"]);
    assert_ok(&out, "why A");
    let why_a = parse_json(&out, "why A json");
    assert_eq!(why_a["status"].as_str(), Some("deleted"));
}

// === TEST 3: `vestige approve --supersedes` promotes and supersedes in one call ===

#[test]
fn approve_with_supersedes_promotes_and_links_in_one_call() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &["note", "add", "Old caching strategy notes", "--json"],
    );
    assert_ok(&out, "note add A");
    let mem_a = parse_json(&out, "note add A json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = vestige(
        &repo,
        &[
            "candidate",
            "add",
            "--type",
            "note",
            "--body",
            "Revised caching strategy notes with TTL guidance",
            "--json",
        ],
    );
    assert_ok(&out, "candidate add");
    let cand_id = parse_json(&out, "candidate add json")["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = vestige(
        &repo,
        &["approve", &cand_id, "--supersedes", &mem_a, "--json"],
    );
    assert_ok(&out, "approve --supersedes");
    let approve_json = parse_json(&out, "approve json");
    let mem_b = approve_json["memory_id"].as_str().unwrap().to_string();

    let out = vestige(&repo, &["why", &mem_a, "--json"]);
    assert_ok(&out, "why A");
    let why_a = parse_json(&out, "why A json");
    assert_eq!(why_a["status"].as_str(), Some("deleted"));
    assert_eq!(
        why_a["provenance"]["superseded_by"].as_str(),
        Some(mem_b.as_str())
    );
}

// === TEST 4: superseding a bogus memory fails loudly, not silently ===

#[test]
fn remember_supersedes_nonexistent_memory_fails() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(
        &repo,
        &[
            "remember",
            "A memory that claims to supersede nothing real",
            "--supersedes",
            "mem_01HTHISISNOTREAL000000000",
            "--json",
        ],
    );
    assert_fail(&out, "remember --supersedes bogus id must fail");
}

// === TEST 5: superseding an already-deleted memory fails loudly ===

#[test]
fn remember_supersedes_already_deleted_memory_fails() {
    let repo = fresh_repo();
    init(&repo);

    let out = vestige(&repo, &["note", "add", "About to be forgotten", "--json"]);
    assert_ok(&out, "note add");
    let mem_a = parse_json(&out, "note add json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = vestige(&repo, &["forget", &mem_a]);
    assert_ok(&out, "forget A");

    let out = vestige(
        &repo,
        &[
            "remember",
            "A memory that claims to supersede an already-forgotten one",
            "--supersedes",
            &mem_a,
            "--json",
        ],
    );
    assert_fail(&out, "remember --supersedes already-deleted id must fail");
}
