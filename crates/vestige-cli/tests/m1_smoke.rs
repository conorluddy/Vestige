//! M1 end-to-end smoke: spawn the built `vestige` binary against a
//! `TempDir`-rooted "repo" with an isolated `~/.vestige` and drive the full
//! capture / list / show / forget / restore lifecycle.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for the package's bins during
    // integration tests.
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
    // Make it look like a git repo so resolution prefers path-hash over
    // searching upwards.
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
fn full_m1_lifecycle() {
    let repo = fresh_repo();

    // === init with summary ===
    let out = vestige(
        &repo,
        &[
            "init",
            "--name",
            "Smoke",
            "--summary",
            "Smoke project for testing.",
        ],
    );
    assert_ok(&out, "init");
    assert!(repo.repo.join(".vestige/config.toml").is_file());

    // === capture: decision ===
    let out = vestige(
        &repo,
        &[
            "decision",
            "add",
            "Use SQLite as the canonical store.",
            "--rationale",
            "Durability + portability.",
            "--json",
        ],
    );
    assert_ok(&out, "decision add");
    let decision = parse_json(&out, "decision json");
    let decision_id = decision["id"].as_str().unwrap().to_string();
    assert!(decision_id.starts_with("mem_"));

    // === capture: note + open question + preference ===
    assert_ok(
        &vestige(
            &repo,
            &["note", "add", "MCP is a thin adapter over the engine."],
        ),
        "note add",
    );
    assert_ok(
        &vestige(
            &repo,
            &["question", "add", "Should embeddings ship in V0.1?"],
        ),
        "question add",
    );
    assert_ok(
        &vestige(&repo, &["preference", "add", "Prefer Markdown PRDs."]),
        "preference add",
    );

    // === remember (defaults to note) ===
    assert_ok(
        &vestige(
            &repo,
            &["remember", "Project memory is per-repo by default."],
        ),
        "remember",
    );

    // === list (json) — expect 5 memories: project_summary + 4 captures + remember ===
    let out = vestige(&repo, &["list", "--json"]);
    assert_ok(&out, "list");
    let cards = parse_json(&out, "list json");
    let cards = cards.as_array().expect("list returns array");
    // project_summary + decision + note + question + preference + remember = 6
    assert_eq!(cards.len(), 6, "expected 6 memories, got {}", cards.len());

    // Type filter: only the decision should match.
    let out = vestige(&repo, &["list", "--type", "decision", "--json"]);
    assert_ok(&out, "list --type decision");
    let cards = parse_json(&out, "list type json");
    let cards = cards.as_array().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["id"].as_str().unwrap(), decision_id);

    // === show --depth full ===
    let out = vestige(&repo, &["show", &decision_id, "--depth", "full"]);
    assert_ok(&out, "show full");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Use SQLite as the canonical store"));
    assert!(stdout.contains("Rationale: Durability + portability"));

    // === forget ===
    assert_ok(&vestige(&repo, &["forget", &decision_id]), "forget");

    // After forget: list should not include the decision.
    let out = vestige(&repo, &["list", "--type", "decision", "--json"]);
    assert_ok(&out, "list after forget");
    let cards = parse_json(&out, "list after forget json");
    assert!(
        cards.as_array().unwrap().is_empty(),
        "decision should be excluded after forget"
    );

    // include_deleted should still surface it.
    let out = vestige(
        &repo,
        &["list", "--type", "decision", "--include-deleted", "--json"],
    );
    assert_ok(&out, "list --include-deleted");
    let cards = parse_json(&out, "list deleted json");
    assert_eq!(cards.as_array().unwrap().len(), 1);

    // === restore ===
    assert_ok(&vestige(&repo, &["restore", &decision_id]), "restore");
    let out = vestige(&repo, &["list", "--type", "decision", "--json"]);
    assert_ok(&out, "list after restore");
    let cards = parse_json(&out, "list after restore json");
    assert_eq!(
        cards.as_array().unwrap().len(),
        1,
        "decision should be visible again"
    );

    // === idempotent re-init does not duplicate the project_summary ===
    assert_ok(
        &vestige(
            &repo,
            &[
                "init",
                "--name",
                "Smoke",
                "--summary",
                "Smoke project for testing.",
            ],
        ),
        "second init",
    );
    let out = vestige(&repo, &["list", "--type", "project_summary", "--json"]);
    assert_ok(&out, "list project_summary");
    let summaries = parse_json(&out, "list summaries json");
    assert_eq!(
        summaries.as_array().unwrap().len(),
        1,
        "project_summary should not be duplicated"
    );

    // Confirm the DB lives under the isolated home, not real ~/.vestige.
    let projects_dir = repo.home.join(".vestige").join("projects");
    let entries: Vec<_> = std::fs::read_dir(&projects_dir).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "exactly one project DB under isolated HOME"
    );
}

#[test]
fn reinit_preserves_config_bytes() {
    // After the first successful `init`, `.vestige/config.toml` belongs to the
    // user. A second `init` must not rewrite it — otherwise hand-added comments
    // or formatting would be silently dropped (toml::to_string_pretty is lossy).
    let repo = fresh_repo();

    assert_ok(&vestige(&repo, &["init", "--name", "Smoke"]), "init");
    let config_path = repo.repo.join(".vestige/config.toml");

    let mut original = std::fs::read_to_string(&config_path).expect("read config");
    original.push_str("\n# user comment that must survive a re-init\n");
    std::fs::write(&config_path, &original).expect("write user comment");

    assert_ok(&vestige(&repo, &["init", "--name", "Smoke"]), "second init");

    let after = std::fs::read_to_string(&config_path).expect("read config after");
    assert_eq!(
        after, original,
        "re-running `init` must not rewrite an existing config.toml"
    );
}

#[test]
fn forget_unknown_id_errors_cleanly() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "x"]), "init");
    let out = vestige(&repo, &["forget", "mem_NOTAREALID"]);
    assert!(!out.status.success(), "forget on unknown id must fail");
}

#[test]
fn list_in_uninit_repo_errors_actionably() {
    let repo = fresh_repo();
    let out = vestige(&repo, &["list"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("vestige init"),
        "stderr should suggest `vestige init`: {stderr}"
    );
}

// Sanity: prove HOME isolation is real — the binary must honour it.
#[test]
fn home_isolation_is_respected() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "iso-test"]), "init");
    let real_home = std::env::var("HOME").unwrap();
    let real_path = Path::new(&real_home)
        .join(".vestige")
        .join("projects")
        .join("proj_iso-test");
    assert!(
        !real_path.exists(),
        "test must not pollute real ~/.vestige (found {real_path:?})"
    );
}
