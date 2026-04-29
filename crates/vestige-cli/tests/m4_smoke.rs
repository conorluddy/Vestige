//! M4 smoke: `vestige context` assembles the pack from stored memories.

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
            "{ctx} failed: {:?}\n{}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn context_pack_contains_all_sections() {
    let repo = fresh_repo();
    assert_ok(
        &vestige(
            &repo,
            &[
                "init",
                "--name",
                "Vestige",
                "--summary",
                "Local-first repo-pinned memory layer.",
            ],
        ),
        "init",
    );
    assert_ok(
        &vestige(
            &repo,
            &["decision", "add", "Use SQLite as the canonical store."],
        ),
        "decision",
    );
    assert_ok(
        &vestige(&repo, &["question", "add", "Embeddings in V0.1 or V0?"]),
        "question",
    );
    assert_ok(
        &vestige(&repo, &["note", "add", "MCP is a thin adapter."]),
        "note",
    );

    // Text mode
    let out = vestige(&repo, &["context"]);
    assert_ok(&out, "context");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Project: Vestige"));
    assert!(stdout.contains("Summary:"));
    assert!(stdout.contains("Local-first"));
    assert!(stdout.contains("Current decisions:"));
    assert!(stdout.contains("Use SQLite"));
    assert!(stdout.contains("Open questions:"));
    assert!(stdout.contains("Embeddings"));
    assert!(stdout.contains("Recent important memories:"));

    // JSON mode
    let out = vestige(&repo, &["context", "--json"]);
    assert_ok(&out, "context json");
    let pack: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        pack["sections"]["project_name"].as_str().unwrap(),
        "Vestige"
    );
    assert_eq!(pack["sections"]["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(
        pack["sections"]["open_questions"].as_array().unwrap().len(),
        1
    );
    assert!(pack["text"].as_str().unwrap().contains("Project: Vestige"));
    assert_eq!(pack["truncated"].as_bool(), Some(false));
}

#[test]
fn budget_truncation_reported() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "Tight"]), "init");
    for i in 0..30 {
        assert_ok(
            &vestige(
                &repo,
                &[
                    "decision",
                    "add",
                    &format!("Decision number {i} which is reasonably wordy filler text."),
                ],
            ),
            "decision",
        );
    }
    let out = vestige(
        &repo,
        &[
            "context",
            "--budget-tokens",
            "30",
            "--per-section",
            "30",
            "--json",
        ],
    );
    assert_ok(&out, "context tight");
    let pack: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(pack["truncated"].as_bool(), Some(true));
    assert!(pack["sections"]["decisions"].as_array().unwrap().len() < 30);
}
