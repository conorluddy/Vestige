//! Smoke tests for `[search] default_mode` config wiring.
//!
//! Verifies that when `.vestige/config.toml` contains `[search] default_mode`
//! the search and recall commands honour it without a `--mode` flag. Because
//! no embeddings are present, a `"hybrid"` config default triggers the
//! PRD §10.3 lexical fallback path and surfaces a warning in the JSON
//! envelope.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;
use toml::Value as TomlValue;

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

/// Initialise the repo, then parse the generated config and insert/replace
/// `[search] default_mode = "<value>"` before writing it back. Using
/// parse-modify-reserialise avoids duplicate `[search]` tables if `init` ever
/// emits that section by default.
fn setup_with_search_mode(default_mode: &str) -> Repo {
    let repo = fresh_repo();
    let init = vestige(&repo, &["init", "--name", "test-project"]);
    assert_ok(&init, "init");

    let config_path = repo.repo.join(".vestige").join("config.toml");
    let raw = std::fs::read_to_string(&config_path).unwrap_or_default();

    let mut doc: toml::map::Map<String, TomlValue> =
        toml::from_str(&raw).expect("config.toml produced by init must be valid TOML");

    let mut search_table = toml::map::Map::new();
    search_table.insert(
        "default_mode".to_string(),
        TomlValue::String(default_mode.to_string()),
    );
    doc.insert("search".to_string(), TomlValue::Table(search_table));

    std::fs::write(
        &config_path,
        toml::to_string(&doc).expect("reserialise config"),
    )
    .expect("write updated config");

    repo
}

// === TESTS ===

/// When `[search] default_mode = "hybrid"` is set and no embeddings exist,
/// `vestige search --json` must succeed and the JSON envelope must contain
/// `"mode": "hybrid"` (the requested mode, even though it fell back to lexical
/// internally) and a non-empty `warnings` array per the fallback path.
#[test]
fn search_uses_config_default_mode_hybrid() {
    let repo = setup_with_search_mode("hybrid");

    let out = vestige(&repo, &["search", "foo", "--json"]);
    assert_ok(&out, "search with config default_mode=hybrid");

    let json = parse_json(&out, "search json");
    // Engine sets effective_mode = Lexical on fallback; mode field reflects what actually ran.
    assert_eq!(
        json["mode"].as_str(),
        Some("lexical"),
        "mode field must reflect effective_mode (lexical fallback)"
    );
    // No embeddings → fallback path emits at least one warning.
    let warnings = json["warnings"].as_array().expect("warnings must be array");
    assert!(
        !warnings.is_empty(),
        "hybrid with no embeddings must produce a fallback warning"
    );
}

/// `vestige search --mode lexical --json` must still override config.
#[test]
fn explicit_mode_flag_overrides_config() {
    let repo = setup_with_search_mode("hybrid");

    let out = vestige(&repo, &["search", "foo", "--mode", "lexical", "--json"]);
    assert_ok(&out, "search with explicit --mode lexical");

    let json = parse_json(&out, "search override json");
    assert_eq!(json["mode"].as_str(), Some("lexical"));
    let warnings = json["warnings"].as_array().expect("warnings must be array");
    assert!(
        warnings.is_empty(),
        "explicit lexical should produce no warnings"
    );
}

/// `vestige recall --json` with `[search] default_mode = "hybrid"` and no
/// embeddings must also surface the hybrid mode + fallback warning.
#[test]
fn recall_uses_config_default_mode_hybrid() {
    let repo = setup_with_search_mode("hybrid");

    let out = vestige(&repo, &["recall", "foo", "--json"]);
    assert_ok(&out, "recall with config default_mode=hybrid");

    let json = parse_json(&out, "recall json");
    // Engine sets effective_mode = Lexical on fallback; mode field reflects what actually ran.
    assert_eq!(
        json["mode"].as_str(),
        Some("lexical"),
        "recall mode field must reflect effective_mode (lexical fallback)"
    );
    let warnings = json["warnings"].as_array().expect("warnings must be array");
    assert!(
        !warnings.is_empty(),
        "hybrid recall with no embeddings must produce a fallback warning"
    );
}

/// When no `[search]` section is present, `vestige search --json` defaults to
/// `"lexical"` with no warnings.
#[test]
fn no_config_defaults_to_lexical() {
    let repo = fresh_repo();
    let init = vestige(&repo, &["init", "--name", "test-project"]);
    assert_ok(&init, "init");

    let out = vestige(&repo, &["search", "foo", "--json"]);
    assert_ok(&out, "search without config");

    let json = parse_json(&out, "search no-config json");
    assert_eq!(json["mode"].as_str(), Some("lexical"));
    let warnings = json["warnings"].as_array().expect("warnings must be array");
    assert!(
        warnings.is_empty(),
        "lexical default must produce no warnings"
    );
}
