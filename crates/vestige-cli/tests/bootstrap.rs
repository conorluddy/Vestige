//! Bootstrap emits a bounded brief without opening the store or running history tools.
use serde_json::Value;
use std::process::{Command, Output};
use tempfile::TempDir;

fn cli(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vestige"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".vestige")).unwrap();
    std::fs::write(
        dir.path().join(".vestige/config.toml"),
        format!("project_id = \"proj_bootstrap\"\nproject_name = \"Bootstrap\"\n[storage]\nmode = \"user_data\"\npath = {:?}\n", dir.path().join("absent.sqlite").to_str().unwrap()),
    ).unwrap();
    dir
}

#[test]
fn brief_resolves_root_from_subdirectory_without_creating_a_store() {
    let dir = repo();
    let child = dir.path().join("src");
    std::fs::create_dir(&child).unwrap();
    let out = cli(
        &child,
        &[
            "bootstrap",
            "--json",
            "--limit",
            "7",
            "--sources",
            "commit",
            "--since",
            "2024-02-29",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let brief: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(brief["project_id"], "proj_bootstrap");
    assert_eq!(
        brief["repo_root"],
        dir.path().canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(brief["dry_run"], false);
    let prompt = brief["prompt"].as_str().unwrap();
    assert!(prompt.contains("--max-count=7"));
    assert!(prompt.contains("--since='2024-02-29'"));
    assert!(!prompt.contains("gh pr list"));
    assert!(!prompt.contains("gh issue list"));
    assert!(prompt.contains("vestige_propose_candidate"));
    assert!(prompt.contains("never auto-approve"));
    assert!(!dir.path().join("absent.sqlite").exists());
    assert!(!dir.path().join(".claude").exists());
}

#[test]
fn preview_forbids_candidate_writes_and_keeps_all_requested_sources() {
    let dir = repo();
    let out = cli(
        dir.path(),
        &["bootstrap", "--dry-run", "--json", "--since", "2026-01-01"],
    );
    assert!(out.status.success());
    let brief: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(brief["dry_run"], true);
    let prompt = brief["prompt"].as_str().unwrap();
    assert!(prompt.contains("PREVIEW ONLY"));
    assert!(prompt.contains("Do not create candidates"));
    assert!(prompt.contains("merged:>=2026-01-01"));
    assert!(prompt.contains("closed:>=2026-01-01"));
    assert!(prompt.contains("git log"));
    assert!(!prompt.contains("Propose each useful finding"));
    assert!(!dir.path().join("absent.sqlite").exists());
}

#[test]
fn missing_project_has_an_actionable_error() {
    let dir = TempDir::new().unwrap();
    let out = cli(dir.path(), &["bootstrap"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("vestige init"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn invalid_dates_limits_and_sources_are_rejected() {
    let dir = repo();
    for args in [
        vec!["bootstrap", "--since", "2025-02-29"],
        vec!["bootstrap", "--since", "2026-01-01;echo secret"],
        vec!["bootstrap", "--limit", "0"],
        vec!["bootstrap", "--limit", "501"],
        vec!["bootstrap", "--sources", "unknown"],
    ] {
        assert!(!cli(dir.path(), &args).status.success(), "{args:?}");
    }
}

#[test]
fn bundled_bootstrap_skill_is_discoverable_and_installs_to_both_targets() {
    let dir = repo();
    let out = cli(dir.path(), &["skills", "list", "--json"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("vestige-bootstrap"));
    let out = cli(dir.path(), &["skills", "install", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for target in [".claude/skills", ".agents/skills"] {
        let installed = dir.path().join(target).join("vestige-bootstrap/SKILL.md");
        assert_eq!(
            std::fs::read_to_string(installed).unwrap(),
            include_str!("../../../skills/vestige/vestige-bootstrap/SKILL.md")
        );
    }
}

#[test]
fn fresh_init_can_emit_a_preview_brief() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vestige"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("VESTIGE_SOCKET", dir.path().join("absent.sock"))
        .args([
            "init",
            "--name",
            "Bootstrap smoke",
            "--no-ui",
            "--no-install-skills",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = cli(&project, &["bootstrap", "--dry-run", "--json"]);
    assert!(out.status.success());
    let brief: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(brief["prompt"].as_str().unwrap().contains("PREVIEW ONLY"));
}
