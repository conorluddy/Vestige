//! V0.4 M1 — `vestige browse` smoke tests.
//!
//! M1 ships scaffolding only. The TUI loop itself is covered by unit tests
//! over `app`, `input`, and `ui` (using `ratatui::backend::TestBackend`). Here
//! we assert two things end-to-end:
//!
//! 1. The subcommand is wired into the clap dispatcher (`browse --help`
//!    succeeds and mentions the `--tab` flag).
//! 2. Running `vestige browse` without a TTY fails fast with the documented
//!    guard message — so a user piping the command never lands in a broken
//!    raw-mode state.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vestige"))
}

#[test]
fn browse_help_lists_tab_flag() {
    let out = Command::new(binary())
        .args(["browse", "--help"])
        .output()
        .expect("vestige binary invoked");
    assert!(out.status.success(), "browse --help should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--tab"),
        "help should mention --tab: {stdout}"
    );
}

#[test]
fn browse_without_tty_errors_cleanly() {
    let tmp = TempDir::new().unwrap();
    let out = Command::new(binary())
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .args(["browse"])
        .output()
        .expect("vestige binary invoked");
    assert!(
        !out.status.success(),
        "browse without a TTY should fail; got success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("needs a TTY") || stderr.contains("TTY"),
        "stderr should mention TTY: {stderr}"
    );
}
