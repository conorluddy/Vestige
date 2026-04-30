//! M7 hybrid search smoke tests.
//!
//! Tests cover the three search modes (lexical / semantic / hybrid) and their
//! edge cases: no-embeddings fallback, JSON envelope shape, score_parts,
//! dedup, and convenience aliases.
//!
//! Tests that depend on `vestige embed --all` (added in PR4) are marked
//! `#[ignore]` with a TODO comment when PR4 hasn't landed yet. Update: PR4
//! is expected to land concurrently — tests are written assuming it is
//! present. If CI fails because the `embed` subcommand doesn't exist, add
//! `#[ignore]` annotations to the four tests that call `embed --all`.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

struct Repo {
    _tmp: TempDir,
    pub repo: PathBuf,
    pub home: PathBuf,
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

/// Append the fake embeddings provider config to the project's config file.
fn append_fake_embeddings_config(repo: &Repo) {
    let config_path = repo.repo.join(".vestige").join("config.toml");
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    if !existing.contains("[embeddings]") {
        std::fs::write(
            &config_path,
            format!("{existing}\n[embeddings]\nprovider = \"fake\"\n"),
        )
        .unwrap();
    }
}

fn seed_memories(repo: &Repo) {
    assert_ok(
        &vestige(
            repo,
            &[
                "decision",
                "add",
                "Use SQLite as the canonical store for all memories.",
            ],
        ),
        "seed decision",
    );
    assert_ok(
        &vestige(
            repo,
            &[
                "note",
                "add",
                "MCP is a thin adapter over the memory engine.",
            ],
        ),
        "seed note",
    );
    assert_ok(
        &vestige(
            repo,
            &["question", "add", "Should we use embeddings in V0.1?"],
        ),
        "seed question",
    );
}

// === TEST 1: lexical mode is unchanged from V0 ===

#[test]
fn lexical_mode_unchanged_from_v0() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "LexicalTest"]), "init");
    seed_memories(&repo);

    let out = vestige(&repo, &["search", "sqlite", "--mode", "lexical", "--json"]);
    assert_ok(&out, "lexical search");
    let envelope = parse_json(&out, "lexical json");

    // New envelope shape.
    assert_eq!(
        envelope["mode"].as_str().unwrap(),
        "lexical",
        "mode field should be lexical"
    );
    let results = envelope["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected at least one lexical result");
    // The decision about SQLite should appear.
    assert!(
        results
            .iter()
            .any(|r| r["type"].as_str() == Some("decision")),
        "expected a decision in results"
    );
    // Score field present, no score_parts for lexical.
    let first = &results[0];
    assert!(first["score"].as_f64().is_some(), "score must be present");
    // score_parts should not be present in lexical results (it's None → skipped in JSON).
    assert!(
        first.get("score_parts").is_none() || first["score_parts"].is_null(),
        "score_parts must be absent for lexical"
    );
}

// === TEST 2: semantic mode with no embeddings shows clear message ===

#[test]
fn semantic_no_embeddings_clear_message() {
    let repo = fresh_repo();
    assert_ok(
        &vestige(&repo, &["init", "--name", "SemanticNoEmbed"]),
        "init",
    );
    append_fake_embeddings_config(&repo);
    seed_memories(&repo);

    // No embed --all run → no embeddings.
    let out = vestige(&repo, &["search", "anything", "--mode", "semantic"]);
    assert_ok(&out, "semantic without embeddings exits 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("embed --all") || stderr.contains("No embeddings"),
        "stderr should hint about embed --all, got: {stderr}"
    );
}

// === TEST 3: semantic mode after embed returns hits ===
// TODO(PR4): depends on `vestige embed --all`. Will be skipped if PR4 hasn't landed.

#[test]
fn semantic_after_embed_returns_hits() {
    let repo = fresh_repo();
    assert_ok(
        &vestige(&repo, &["init", "--name", "SemanticAfterEmbed"]),
        "init",
    );
    append_fake_embeddings_config(&repo);
    seed_memories(&repo);

    assert_ok(&vestige(&repo, &["embed", "--all"]), "embed --all");

    let out = vestige(&repo, &["search", "store", "--mode", "semantic", "--json"]);
    assert_ok(&out, "semantic search after embed");
    let envelope = parse_json(&out, "semantic json");
    assert_eq!(envelope["mode"].as_str().unwrap(), "semantic");
    let results = envelope["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected at least one semantic hit");
    assert!(
        results[0]["score"].as_f64().unwrap() >= 0.0,
        "score must be non-negative"
    );
}

// === TEST 4: hybrid mode returns score_parts ===
// TODO(PR4): depends on `vestige embed --all`.

#[test]
fn hybrid_returns_score_parts() {
    let repo = fresh_repo();
    assert_ok(
        &vestige(&repo, &["init", "--name", "HybridScoreParts"]),
        "init",
    );
    append_fake_embeddings_config(&repo);
    seed_memories(&repo);

    assert_ok(&vestige(&repo, &["embed", "--all"]), "embed --all");

    let out = vestige(&repo, &["search", "store", "--mode", "hybrid", "--json"]);
    assert_ok(&out, "hybrid search");
    let envelope = parse_json(&out, "hybrid json");
    assert_eq!(envelope["mode"].as_str().unwrap(), "hybrid");
    assert!(
        envelope["warnings"].as_array().unwrap().is_empty(),
        "no warnings expected when embeddings exist"
    );
    let results = envelope["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected hybrid results");

    // Every hybrid result must have score_parts.
    for result in results {
        let parts = result
            .get("score_parts")
            .expect("score_parts must be present for hybrid");
        assert!(
            parts["fts"].as_f64().is_some(),
            "score_parts.fts must be a number"
        );
        assert!(
            parts["vector"].as_f64().is_some(),
            "score_parts.vector must be a number"
        );
        assert!(
            parts["importance"].as_f64().is_some(),
            "score_parts.importance must be a number"
        );
        assert!(
            parts["type_boost"].as_f64().is_some(),
            "score_parts.type_boost must be a number"
        );
        assert!(
            parts["total"].as_f64().is_some(),
            "score_parts.total must be a number"
        );
    }
}

// === TEST 5: hybrid falls back to lexical with warning when no embeddings ===

#[test]
fn hybrid_falls_back_lexical_with_warning() {
    let repo = fresh_repo();
    assert_ok(
        &vestige(&repo, &["init", "--name", "HybridFallback"]),
        "init",
    );
    append_fake_embeddings_config(&repo);
    seed_memories(&repo);

    // No embed --all → no embeddings. Hybrid should fall back to lexical.
    let out = vestige(&repo, &["search", "store", "--mode", "hybrid", "--json"]);
    assert_ok(&out, "hybrid fallback exits 0");
    let envelope = parse_json(&out, "hybrid fallback json");
    assert_eq!(envelope["mode"].as_str().unwrap(), "hybrid");

    let warnings = envelope["warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty(),
        "warnings must be present on lexical fallback"
    );
    assert!(
        warnings[0].as_str().unwrap().contains("lexical")
            || warnings[0].as_str().unwrap().contains("embed"),
        "warning should mention lexical fallback or embed, got: {}",
        warnings[0]
    );

    // Results should still be returned (lexical fallback).
    let results = envelope["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "fallback lexical results must be returned"
    );
}

// === TEST 6: hybrid dedup — no duplicate ids ===
// TODO(PR4): depends on `vestige embed --all`.

#[test]
fn hybrid_dedup_no_duplicates() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "HybridDedup"]), "init");
    append_fake_embeddings_config(&repo);

    // Record a memory very likely to appear in both lexical and semantic legs.
    assert_ok(
        &vestige(
            &repo,
            &[
                "decision",
                "add",
                "Use SQLite store with FTS5. The SQLite store is the canonical memory store.",
            ],
        ),
        "seed SQLite decision",
    );

    assert_ok(&vestige(&repo, &["embed", "--all"]), "embed --all");

    let out = vestige(
        &repo,
        &["search", "SQLite store", "--mode", "hybrid", "--json"],
    );
    assert_ok(&out, "hybrid dedup search");
    let envelope = parse_json(&out, "hybrid dedup json");
    let results = envelope["results"].as_array().unwrap();

    // Collect all ids; they must be unique.
    let ids: Vec<&str> = results.iter().filter_map(|r| r["id"].as_str()).collect();
    let unique_ids: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique_ids.len(),
        "duplicate IDs found in hybrid results: {ids:?}",
    );
}

// === TEST 7: convenience aliases work ===

#[test]
fn convenience_aliases_work() {
    let repo = fresh_repo();
    assert_ok(&vestige(&repo, &["init", "--name", "Aliases"]), "init");
    seed_memories(&repo);

    // --lexical alias should work the same as --mode lexical.
    let out_alias = vestige(&repo, &["search", "sqlite", "--lexical", "--json"]);
    assert_ok(&out_alias, "search --lexical alias");
    let env_alias = parse_json(&out_alias, "alias json");
    assert_eq!(env_alias["mode"].as_str().unwrap(), "lexical");

    // --hybrid alias without embeddings should fall back to lexical (same as --mode hybrid).
    let out_hybrid = vestige(&repo, &["search", "store", "--hybrid", "--json"]);
    assert_ok(&out_hybrid, "search --hybrid alias exits 0");
    let env_hybrid = parse_json(&out_hybrid, "hybrid alias json");
    assert_eq!(env_hybrid["mode"].as_str().unwrap(), "hybrid");
    // Without embeddings the warnings array must be non-empty.
    assert!(
        !env_hybrid["warnings"].as_array().unwrap().is_empty(),
        "expected fallback warning for --hybrid with no embeddings"
    );
}

// === Bonus: verify --help shows the new flags ===

#[test]
fn search_help_shows_mode_flags() {
    let repo = fresh_repo();
    // We just need a dir; init isn't required for --help.
    let out = Command::new(binary())
        .current_dir(&repo.repo)
        .env("HOME", &repo.home)
        .args(["search", "--help"])
        .output()
        .expect("help invoked");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--mode") || stdout.contains("mode"),
        "--mode flag must appear in search --help"
    );
    assert!(
        stdout.contains("--hybrid"),
        "--hybrid alias must appear in search --help"
    );
    assert!(
        stdout.contains("--semantic"),
        "--semantic alias must appear in search --help"
    );
}
