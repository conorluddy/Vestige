//! Integration smoke tests for `vestige skills install` and the skills-install
//! path wired into `vestige init`.
//!
//! Every test spawns the real `vestige` binary against isolated TempDirs —
//! no mocking, no shared state. Pattern copied from `crates/vestige-cli/tests/m1_smoke.rs`.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;

// === HELPERS ===

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

fn run_vestige(args: &[&str], cwd: &Path, home: &Path) -> Output {
    std::process::Command::new(binary())
        .current_dir(cwd)
        .env("HOME", home)
        .env("VESTIGE_LOG", "warn")
        .args(args)
        .output()
        .expect("vestige binary invoked")
}

fn assert_ok(out: &Output, ctx: &str) {
    if !out.status.success() {
        panic!(
            "{ctx} failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn parse_json(out: &Output, ctx: &str) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{ctx} not JSON: {e}\n{stdout}"))
}

/// `vestige skills install` returns `{ "results": [<per-target report>] }`.
/// When called with `--dest <path>` there's exactly one entry; this helper
/// returns it for tests that exercise the single-target path.
fn first_result(envelope: &Value) -> &Value {
    envelope["results"]
        .as_array()
        .and_then(|results| results.first())
        .unwrap_or_else(|| panic!("expected results array with >= 1 entry; got: {envelope}"))
}

struct Dirs {
    _tmp: TempDir,
    cwd: PathBuf,
    home: PathBuf,
    dest: PathBuf,
}

/// Fresh isolated environment: cwd (with .git), home, and a separate install dest.
fn fresh_dirs() -> Dirs {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let dest = tmp.path().join("skills-dest");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    // Make it look like a git repo so project-id resolution is deterministic.
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    Dirs {
        _tmp: tmp,
        cwd,
        home,
        dest,
    }
}

// === TESTS ===

/// Install writes all bundled skill files to the dest directory.
#[test]
fn skills_install_writes_files_to_tmpdir() {
    let dirs = fresh_dirs();
    let dest_str = dirs.dest.to_string_lossy().to_string();

    let out = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out, "skills install");

    let envelope = parse_json(&out, "skills install json");
    let report = first_result(&envelope);

    // At least 10 skills × 3 files each = 30; real count is higher.
    let written = report["written"].as_array().unwrap();
    assert!(
        written.len() >= 30,
        "expected >= 30 written files, got {}",
        written.len()
    );

    // Drifted must be empty on a clean install.
    let drifted = report["drifted"].as_array().unwrap();
    assert!(
        drifted.is_empty(),
        "expected no drifted files on first install"
    );

    // The canonical entry point for one skill must exist on disk.
    let skill_md = dirs.dest.join("vestige-auto-memorise").join("SKILL.md");
    assert!(
        skill_md.is_file(),
        "vestige-auto-memorise/SKILL.md must exist after install"
    );
    let contents = std::fs::read(&skill_md).unwrap();
    assert!(!contents.is_empty(), "SKILL.md must not be empty");
}

/// Running install twice with no edits: second run reports nothing written.
#[test]
fn skills_install_is_idempotent() {
    let dirs = fresh_dirs();
    let dest_str = dirs.dest.to_string_lossy().to_string();

    let out1 = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out1, "first install");
    let env1 = parse_json(&out1, "first install json");
    let r1 = first_result(&env1);
    let written_count = r1["written"].as_array().unwrap().len();

    let out2 = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out2, "second install");
    let env2 = parse_json(&out2, "second install json");
    let r2 = first_result(&env2);

    assert!(
        r2["written"].as_array().unwrap().is_empty(),
        "second install should write nothing"
    );
    assert_eq!(
        r2["skipped"].as_array().unwrap().len(),
        written_count,
        "second install should skip exactly what the first wrote"
    );
    assert!(
        r2["drifted"].as_array().unwrap().is_empty(),
        "no drift expected on idempotent re-install"
    );
}

/// Drift in a skill file causes install to hard-fail (non-zero exit).
/// The drifted file must not be overwritten.
#[test]
fn skills_install_drift_causes_hard_fail() {
    let dirs = fresh_dirs();
    let dest_str = dirs.dest.to_string_lossy().to_string();

    // First install populates the dest.
    let out1 = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out1, "first install");

    // Tamper with a file to simulate a local edit.
    let drifted_file = dirs.dest.join("vestige-recall").join("SKILL.md");
    let original_bytes = std::fs::read(&drifted_file).unwrap();
    let mut tampered = original_bytes.clone();
    tampered.push(b'X');
    std::fs::write(&drifted_file, &tampered).unwrap();

    // Second install must fail and report drift.
    let out2 = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert!(
        !out2.status.success(),
        "install with drift should exit non-zero; status={:?}\nstdout:\n{}\nstderr:\n{}",
        out2.status,
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr),
    );

    // JSON output must name the drifted path.
    let env2 = parse_json(&out2, "drift install json");
    let r2 = first_result(&env2);
    let drifted = r2["drifted"].as_array().unwrap();
    assert!(
        drifted
            .iter()
            .any(|p| p.as_str().unwrap_or("").contains("vestige-recall/SKILL.md")),
        "drifted array must contain vestige-recall/SKILL.md; got: {drifted:?}"
    );

    // The tampered file must still contain our appended byte — not overwritten.
    let on_disk = std::fs::read(&drifted_file).unwrap();
    assert_eq!(
        on_disk, tampered,
        "drifted file must not be overwritten without --force"
    );
}

/// `--force` overwrites drifted files and exits zero.
#[test]
fn skills_install_force_overwrites_drift() {
    let dirs = fresh_dirs();
    let dest_str = dirs.dest.to_string_lossy().to_string();

    // First install.
    let out1 = run_vestige(
        &["skills", "install", "--dest", &dest_str, "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out1, "first install");

    // Tamper with the same file as in the drift test.
    let drifted_file = dirs.dest.join("vestige-recall").join("SKILL.md");
    let bundled_bytes = std::fs::read(&drifted_file).unwrap();
    std::fs::write(&drifted_file, b"local edit").unwrap();

    // Force re-install — must succeed.
    let out2 = run_vestige(
        &[
            "skills", "install", "--dest", &dest_str, "--force", "--json",
        ],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out2, "force install");

    let env2 = parse_json(&out2, "force install json");
    let r2 = first_result(&env2);

    // No drift in the report.
    assert!(
        r2["drifted"].as_array().unwrap().is_empty(),
        "drifted must be empty after --force"
    );

    // The formerly-drifted path must appear in written.
    let written = r2["written"].as_array().unwrap();
    assert!(
        written
            .iter()
            .any(|p| p.as_str().unwrap_or("").contains("vestige-recall/SKILL.md")),
        "written must include the formerly-drifted file; got: {written:?}"
    );

    // File content is now the bundled bytes (not "local edit").
    let on_disk = std::fs::read(&drifted_file).unwrap();
    assert_eq!(
        on_disk, bundled_bytes,
        "file must match bundled bytes after --force"
    );
}

/// `vestige init` installs skills to BOTH `.claude/skills/` and `.agents/skills/` by default.
#[test]
fn init_installs_skills_to_both_targets_by_default() {
    let dirs = fresh_dirs();

    let out = run_vestige(&["init", "--json"], &dirs.cwd, &dirs.home);
    assert_ok(&out, "init");

    let envelope = parse_json(&out, "init json");
    let results = envelope["skills_installed"]["results"]
        .as_array()
        .expect("skills_installed.results must be an array after default init");
    assert_eq!(
        results.len(),
        2,
        "default init must report two install targets; got: {envelope}"
    );

    let mut targets: Vec<&str> = results
        .iter()
        .map(|r| r["target"].as_str().unwrap())
        .collect();
    targets.sort();
    assert_eq!(targets, vec!["agents", "claude"]);

    for r in results {
        let written = r["written"].as_u64().unwrap_or(0);
        assert!(written > 0, "each target must write skills; got: {r}");
        assert_eq!(r["drifted"].as_u64().unwrap(), 0);
    }

    // Both dirs must exist on disk with the canonical SKILL.md.
    for sub in [".claude", ".agents"] {
        let skill_md = dirs
            .cwd
            .join(sub)
            .join("skills")
            .join("vestige-auto-memorise")
            .join("SKILL.md");
        assert!(skill_md.is_file(), "SKILL.md must exist at {skill_md:?}");
    }
}

/// `vestige init --skills-target claude` only writes to `.claude/skills/`.
#[test]
fn init_skills_target_claude_only() {
    let dirs = fresh_dirs();

    let out = run_vestige(
        &["init", "--skills-target", "claude", "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out, "init --skills-target claude");

    let envelope = parse_json(&out, "init claude-only json");
    let results = envelope["skills_installed"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target"].as_str().unwrap(), "claude");

    assert!(dirs.cwd.join(".claude").join("skills").is_dir());
    assert!(
        !dirs.cwd.join(".agents").exists(),
        ".agents/ must not exist when targeting claude only"
    );
}

/// `vestige init --no-install-skills` omits the skills step entirely.
#[test]
fn init_no_install_skills_skips_install() {
    let dirs = fresh_dirs();

    let out = run_vestige(
        &["init", "--no-install-skills", "--json"],
        &dirs.cwd,
        &dirs.home,
    );
    assert_ok(&out, "init --no-install-skills");

    let envelope = parse_json(&out, "init --no-install-skills json");

    assert!(
        envelope["skills_installed"].is_null(),
        "skills_installed must be null when --no-install-skills is set; envelope: {envelope}"
    );

    // Neither target dir must exist on disk.
    for sub in [".claude", ".agents"] {
        let skills_dir = dirs.cwd.join(sub).join("skills");
        assert!(
            !skills_dir.exists(),
            "{sub}/skills/ must not exist when skills install is skipped"
        );
    }
}

/// `vestige init --dry-run` does not install skills.
#[test]
fn init_dry_run_does_not_install_skills() {
    let dirs = fresh_dirs();

    let out = run_vestige(&["init", "--dry-run", "--json"], &dirs.cwd, &dirs.home);
    assert_ok(&out, "init --dry-run");

    let envelope = parse_json(&out, "init --dry-run json");

    assert!(
        envelope["skills_installed"].is_null(),
        "skills_installed must be null in dry-run mode; envelope: {envelope}"
    );

    // Neither target dir must exist on disk.
    for sub in [".claude", ".agents"] {
        let skills_dir = dirs.cwd.join(sub).join("skills");
        assert!(
            !skills_dir.exists(),
            "{sub}/skills/ must not exist after --dry-run"
        );
    }
}
