//! M6 smoke tests for `vestige embed`, `vestige embeddings status`, and
//! `vestige reindex`.
//!
//! All tests use an isolated `HOME` so they never touch the real
//! `~/.vestige`. A `.git` directory makes the project ID stable.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

// === TEST HARNESS ===

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

/// After `vestige init`, append a `[embeddings]` section so the provider resolves
/// to `fake` (which is the compile-time default anyway, but being explicit lets
/// tests document the intended configuration).
fn write_embeddings_config(repo: &Repo) {
    let config_path = repo.repo.join(".vestige/config.toml");
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    if !existing.contains("[embeddings]") {
        let appended = format!("{existing}\n[embeddings]\nprovider = \"fake\"\n");
        std::fs::write(&config_path, appended).unwrap();
    }
}

// === TESTS ===

#[test]
fn embed_dry_run_lists_targets() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "EmbedDryRun"]), "init");
    write_embeddings_config(&repo);

    // Add 2 decisions — each gets summary + compressed representations.
    assert_ok(
        &vestige(&repo, &["decision", "add", "Use SQLite for durability."]),
        "decision 1",
    );
    assert_ok(
        &vestige(&repo, &["decision", "add", "Use ULIDs for sortable IDs."]),
        "decision 2",
    );

    let out = vestige(&repo, &["embed", "--all", "--dry-run", "--json"]);
    assert_ok(&out, "embed --dry-run");

    let json = parse_json(&out, "embed dry-run json");

    assert!(
        json["dry_run"].as_bool() == Some(true),
        "dry_run must be true"
    );

    // 2 decisions × 2 default representations = 4 in embedded (would_embed)
    // plus whatever the init project_summary produces.
    let embedded = json["embedded"].as_array().unwrap();
    // At least 4 would_embed targets (2 decisions × summary + compressed).
    assert!(
        embedded.len() >= 4,
        "expected at least 4 would_embed targets, got {}",
        embedded.len()
    );

    for t in embedded {
        assert_eq!(
            t["action"].as_str(),
            Some("would_embed"),
            "all targets in dry-run must have action=would_embed"
        );
    }

    // Verify nothing was actually written — embeddings status must show 0 embedded.
    let status_out = vestige(&repo, &["embeddings", "status", "--json"]);
    assert_ok(&status_out, "embeddings status after dry-run");
    let status = parse_json(&status_out, "status json");
    assert_eq!(
        status["embedded_representations"].as_u64(),
        Some(0),
        "no embeddings should be written during dry-run"
    );
}

#[test]
fn embed_all_then_status_shows_counts() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "EmbedAll"]), "init");
    write_embeddings_config(&repo);

    assert_ok(
        &vestige(
            &repo,
            &["decision", "add", "Use async Rust at the MCP layer."],
        ),
        "decision",
    );
    assert_ok(
        &vestige(
            &repo,
            &["note", "add", "Keep CLI thin — no business logic."],
        ),
        "note",
    );
    assert_ok(
        &vestige(
            &repo,
            &["preference", "add", "Prefer explicit error types."],
        ),
        "preference",
    );

    let embed_out = vestige(&repo, &["embed", "--all", "--json"]);
    assert_ok(&embed_out, "embed --all");

    let embed_json = parse_json(&embed_out, "embed json");
    assert_eq!(embed_json["dry_run"].as_bool(), Some(false));
    assert!(
        embed_json["failed"].as_array().unwrap().is_empty(),
        "no embeddings should fail with the fake provider"
    );

    let status_out = vestige(&repo, &["embeddings", "status", "--json"]);
    assert_ok(&status_out, "embeddings status");
    let status = parse_json(&status_out, "status json");

    let embedded = status["embedded_representations"].as_u64().unwrap_or(0);
    // 3 memories × 2 depths = 6, plus project_summary init = 2 more = 8 minimum.
    assert!(
        embedded >= 6,
        "expected at least 6 embedded representations, got {embedded}"
    );
    assert_eq!(
        status["failed_jobs"].as_u64(),
        Some(0),
        "no failed jobs expected"
    );
}

#[test]
fn reindex_embeddings_is_idempotent() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "ReindexIdem"]), "init");
    write_embeddings_config(&repo);

    assert_ok(
        &vestige(
            &repo,
            &[
                "note",
                "add",
                "Embeddings are a disposable acceleration layer.",
            ],
        ),
        "note",
    );

    // Embed once.
    assert_ok(&vestige(&repo, &["embed", "--all"]), "embed 1");

    let status1 = {
        let out = vestige(&repo, &["embeddings", "status", "--json"]);
        assert_ok(&out, "status 1");
        parse_json(&out, "status 1 json")
    };

    // Reindex --embeddings (clears + re-embeds).
    let reindex_out = vestige(&repo, &["reindex", "--embeddings", "--json"]);
    assert_ok(&reindex_out, "reindex --embeddings");

    let status2 = {
        let out = vestige(&repo, &["embeddings", "status", "--json"]);
        assert_ok(&out, "status 2");
        parse_json(&out, "status 2 json")
    };

    assert_eq!(
        status1["embedded_representations"], status2["embedded_representations"],
        "embedded count must be identical after reindex"
    );
    assert_eq!(
        status2["failed_jobs"].as_u64(),
        Some(0),
        "reindex must not leave failed jobs"
    );
}

#[test]
fn embed_excludes_forgotten_memories() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "ForgetEmbed"]), "init");
    write_embeddings_config(&repo);

    assert_ok(
        &vestige(
            &repo,
            &["decision", "add", "This decision will be forgotten."],
        ),
        "decision",
    );

    // Embed everything.
    assert_ok(&vestige(&repo, &["embed", "--all"]), "embed");

    let status1 = {
        let out = vestige(&repo, &["embeddings", "status", "--json"]);
        assert_ok(&out, "status before forget");
        parse_json(&out, "status before forget json")
    };
    let embedded_before = status1["embedded_representations"].as_u64().unwrap_or(0);

    // Forget the decision — the soft-delete trigger marks embeddings stale.
    let list_out = vestige(&repo, &["list", "--type", "decision", "--json"]);
    assert_ok(&list_out, "list decisions");
    let decisions = parse_json(&list_out, "list json")
        .as_array()
        .unwrap()
        .to_vec();
    assert!(!decisions.is_empty(), "must have at least one decision");
    let decision_id = decisions[0]["id"].as_str().unwrap().to_string();

    assert_ok(&vestige(&repo, &["forget", &decision_id]), "forget");

    let status2 = {
        let out = vestige(&repo, &["embeddings", "status", "--json"]);
        assert_ok(&out, "status after forget");
        parse_json(&out, "status after forget json")
    };

    // After forget: stale_embeddings should be > 0 (the trigger fired),
    // and embedded_representations should be less than before.
    let stale_after = status2["stale_embeddings"].as_u64().unwrap_or(0);
    let embedded_after = status2["embedded_representations"].as_u64().unwrap_or(0);

    assert!(
        stale_after > 0,
        "soft-delete trigger must mark embeddings stale"
    );
    assert!(
        embedded_after < embedded_before,
        "embedded count must decrease after forget (was {embedded_before}, now {embedded_after})"
    );
}

#[test]
fn embed_unknown_memory_id_errors() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "BadId"]), "init");

    let out = vestige(&repo, &["embed", "--memory", "mem_NOTAREALID"]);
    assert!(
        !out.status.success(),
        "embedding a non-existent memory id must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Should mention the bad ID or "not found" in the error chain.
    assert!(
        stderr.contains("mem_NOTAREALID")
            || stderr.contains("not found")
            || stderr.contains("invalid"),
        "error message should reference the bad id or say not found: {stderr}"
    );
}
