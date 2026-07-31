//! `vestige revise` end-to-end smoke (issue #130): spawn the built binary
//! against a `TempDir`-rooted "repo" with an isolated `~/.vestige` and drive
//! `remember → revise → search (old gone, new found) → why (memory.revised)`.

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
            "{ctx} failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn parse_json(out: &std::process::Output, ctx: &str) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{ctx} not JSON: {e}\n{stdout}"))
}

#[test]
fn remember_revise_search_why_lifecycle() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "Revise"]), "init");

    // === remember ===
    let out = vestige(
        &repo,
        &["remember", "The garbanzo pipeline runs nightly.", "--json"],
    );
    assert_ok(&out, "remember");
    let captured = parse_json(&out, "remember json");
    let id = captured["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("mem_"));

    // Sanity: the old token is searchable before any revision. `search --json`
    // emits the PRD §12.6 envelope `{ mode, results, warnings }`, not a bare array.
    let out = vestige(&repo, &["search", "garbanzo", "--json"]);
    assert_ok(&out, "search garbanzo before revise");
    let hits = parse_json(&out, "search garbanzo before revise json");
    assert_eq!(hits["results"].as_array().unwrap().len(), 1);

    // === revise ===
    let out = vestige(
        &repo,
        &[
            "revise",
            &id,
            "The falafel pipeline runs hourly now.",
            "--json",
        ],
    );
    assert_ok(&out, "revise");
    let revised = parse_json(&out, "revise json");
    assert_eq!(revised["id"].as_str().unwrap(), id);
    assert!(revised["prior_one_liner"]
        .as_str()
        .unwrap()
        .contains("garbanzo"));
    assert!(revised["one_liner"].as_str().unwrap().contains("falafel"));

    // === search: old token gone, new token found ===
    let out = vestige(&repo, &["search", "garbanzo", "--json"]);
    assert_ok(&out, "search garbanzo after revise");
    let hits = parse_json(&out, "search garbanzo after revise json");
    assert!(
        hits["results"].as_array().unwrap().is_empty(),
        "old token must not match after revision"
    );

    let out = vestige(&repo, &["search", "falafel", "--json"]);
    assert_ok(&out, "search falafel after revise");
    let hits = parse_json(&out, "search falafel after revise json");
    let hits = hits["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "new token must match after revision");
    assert_eq!(hits[0]["id"].as_str().unwrap(), id);

    // === why: memory.revised is visible in the provenance walk ===
    let out = vestige(&repo, &["why", &id]);
    assert_ok(&out, "why");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("memory.revised"),
        "why output should surface the memory.revised event:\n{stdout}"
    );
}

#[test]
fn revise_unknown_id_errors_cleanly() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "x"]), "init");
    let out = vestige(&repo, &["revise", "mem_NOTAREALID", "new body"]);
    assert!(!out.status.success(), "revise on unknown id must fail");
}

#[test]
fn revise_deleted_memory_errors_cleanly() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "x"]), "init");

    let out = vestige(&repo, &["remember", "Soon to be forgotten.", "--json"]);
    assert_ok(&out, "remember");
    let id = parse_json(&out, "remember json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ok(&vestige(&repo, &["forget", &id]), "forget");

    let out = vestige(&repo, &["revise", &id, "Should never land."]);
    assert!(
        !out.status.success(),
        "revise on a deleted memory must fail rather than silently succeed"
    );
}

#[test]
fn revise_empty_body_errors_cleanly() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "x"]), "init");

    let out = vestige(&repo, &["remember", "Has a body for now.", "--json"]);
    assert_ok(&out, "remember");
    let id = parse_json(&out, "remember json")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = vestige(&repo, &["revise", &id, "   "]);
    assert!(
        !out.status.success(),
        "revise with a whitespace-only body must fail validation"
    );
}
