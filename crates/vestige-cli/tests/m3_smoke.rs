//! M3 smoke: `vestige search` and `vestige recall` against the built binary.

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
fn search_and_recall_roundtrip() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "Search"]), "init");

    assert_ok(
        &vestige(
            &repo,
            &[
                "decision",
                "add",
                "Use SQLite as the canonical store for memories.",
            ],
        ),
        "decision add",
    );
    assert_ok(
        &vestige(
            &repo,
            &["note", "add", "MCP is a thin adapter over the engine."],
        ),
        "note add",
    );
    assert_ok(
        &vestige(&repo, &["question", "add", "Embeddings in V0.1 or V0?"]),
        "question add",
    );

    // === search returns scored cards in envelope {mode, results, warnings} ===
    let out = vestige(&repo, &["search", "SQLite", "--json"]);
    assert_ok(&out, "search SQLite");
    let envelope = parse_json(&out, "search json");
    assert_eq!(envelope["mode"].as_str().unwrap(), "lexical");
    let arr = envelope["results"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "expected one match for SQLite");
    let hit = &arr[0];
    assert_eq!(hit["type"].as_str().unwrap(), "decision");
    assert!(
        hit["score"].as_f64().unwrap() > 0.0,
        "composite score should be positive"
    );

    // === decision boosted above note for an equally-matching term ===
    // Both records contain "engine"-adjacent and "store"-adjacent text;
    // search for a token present in both bodies.
    assert_ok(
        &vestige(
            &repo,
            &["note", "add", "The store will use SQLite for persistence."],
        ),
        "competing note",
    );
    let out = vestige(&repo, &["search", "SQLite", "--json"]);
    assert_ok(&out, "search SQLite again");
    let envelope2 = parse_json(&out, "search json 2");
    let arr = envelope2["results"].as_array().unwrap();
    assert!(arr.len() >= 2, "expected at least 2 hits");
    // The decision (boosted) should still be at or near the top.
    assert_eq!(
        arr[0]["type"].as_str().unwrap(),
        "decision",
        "decision should rank first via type boost"
    );

    // === type filter narrows search ===
    let out = vestige(&repo, &["search", "SQLite", "--type", "note", "--json"]);
    assert_ok(&out, "search type=note");
    let env_note = parse_json(&out, "search type=note json");
    let arr = env_note["results"].as_array().unwrap();
    assert!(arr.iter().all(|h| h["type"].as_str() == Some("note")));

    // === recall behaves like search ===
    let out = vestige(&repo, &["recall", "MCP adapter", "--json"]);
    assert_ok(&out, "recall");
    let env_recall = parse_json(&out, "recall json");
    let arr = env_recall["results"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"].as_str().unwrap(), "note");

    // === forget excludes from search ===
    let out = vestige(&repo, &["list", "--type", "decision", "--json"]);
    let id = parse_json(&out, "list decisions")[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ok(&vestige(&repo, &["forget", &id]), "forget");

    let out = vestige(&repo, &["search", "SQLite", "--json"]);
    assert_ok(&out, "search after forget");
    let env_af = parse_json(&out, "search after forget json");
    assert!(
        env_af["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["id"].as_str() != Some(id.as_str())),
        "forgotten decision must not appear in search"
    );

    // === restore puts it back ===
    assert_ok(&vestige(&repo, &["restore", &id]), "restore");
    let out = vestige(&repo, &["search", "SQLite", "--json"]);
    assert_ok(&out, "search after restore");
    let env_ar = parse_json(&out, "search after restore json");
    assert!(env_ar["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["id"].as_str() == Some(id.as_str())));
}

#[test]
fn empty_query_returns_no_matches_text_mode() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "x"]), "init");
    assert_ok(&vestige(&repo, &["note", "add", "anything"]), "note");

    let out = vestige(&repo, &["search", "***"]);
    assert_ok(&out, "search ***");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(no matches)"));
}

#[test]
fn search_in_uninit_repo_errors_actionably() {
    let repo = fresh_repo();
    let out = vestige(&repo, &["search", "anything"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("vestige init"));
}
