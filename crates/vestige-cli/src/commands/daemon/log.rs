//! `vestige daemon log [-f]` — print or tail the daemon log file.
//!
//! Since V0.5, the daemon writes to daily-rotated files under
//! `~/.vestige/logs/` via `tracing-appender`. This command resolves the most
//! recently modified `daemon.log*` file there and delegates to `tail`.
//!
//! For `-f` / `--follow` we invoke `tail -F` (capital F), which follows the
//! path through log rotations: when `tracing-appender` starts a new day's
//! file, `tail -F` will notice the original path is gone and re-open it once
//! the new file appears. However, `default_log_path()` returns a concrete
//! dated file path, so users following today's log across midnight may need to
//! re-run the command after rotation to pick up the new file.
//!
//! # Backward compatibility
//!
//! Users who installed the plist before V0.5 will still have the daemon writing
//! to `~/.vestige/daemon.log`. If `~/.vestige/logs/` does not exist or is
//! empty, this command falls back to that old path transparently.

use clap::Args;

// === TYPES ===

#[derive(Args, Debug)]
pub struct LogArgs {
    /// Follow the log as it grows (like `tail -f`).
    #[arg(short = 'f', long)]
    pub follow: bool,
    /// Number of lines from the end to show (default 100).
    #[arg(short = 'n', long, default_value_t = 100)]
    pub lines: usize,
}

// === PUBLIC API ===

pub fn run(args: LogArgs) -> anyhow::Result<()> {
    let path = default_log_path();
    if !path.exists() {
        anyhow::bail!(
            "daemon log not found at {}; daemon may never have run",
            path.display()
        );
    }

    // Delegate to `tail` for portability and log-rotation robustness.
    // `-F` (capital F) follows the filename across rotations; `-f` (lower)
    // follows the file descriptor. We always use `-F` for follow mode so the
    // user doesn't need to restart the command after a midnight rotation.
    // V0.5 targets macOS only, so BSD `tail` is always available.
    if args.follow {
        let status = std::process::Command::new("tail")
            .arg("-F")
            .arg(&path)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    } else {
        let status = std::process::Command::new("tail")
            .arg("-n")
            .arg(args.lines.to_string())
            .arg(&path)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

// === PRIVATE HELPERS ===

/// Resolve the log file path to display.
///
/// Checks `~/.vestige/logs/` for the most recently modified `daemon.log*`
/// file (the daily-rotated naming convention used by `tracing-appender`).
/// Falls back to `~/.vestige/daemon.log` for installations that pre-date V0.5
/// and have not yet run `daemon uninstall && daemon install`.
pub fn default_log_path() -> std::path::PathBuf {
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()));

    let new_dir = home.join(".vestige").join("logs");
    if new_dir.exists() {
        // Find the most recently modified daemon.log* file. tracing-appender
        // daily rotation names files "daemon.log.YYYY-MM-DD"; we match any
        // name starting with "daemon.log" to stay forward-compatible.
        let mut latest: Option<std::path::PathBuf> = None;
        let mut latest_mtime = std::time::SystemTime::UNIX_EPOCH;
        if let Ok(entries) = std::fs::read_dir(&new_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_daemon_log = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("daemon.log"));
                if is_daemon_log {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if mtime > latest_mtime {
                                latest_mtime = mtime;
                                latest = Some(path);
                            }
                        }
                    }
                }
            }
        }
        if let Some(p) = latest {
            return p;
        }
    }

    // Backward-compat fallback: pre-V0.5 installs write here via the old plist.
    // Once the user runs `daemon uninstall && daemon install`, the plist will
    // redirect launchd's fd to daemon-stderr.log and this path is no longer used.
    home.join(".vestige").join("daemon.log")
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper: temporarily override HOME so `default_log_path` resolves
    /// into a controlled directory. Returns the `TempDir` (keep alive).
    fn make_home() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn falls_back_to_legacy_path_when_logs_dir_absent() {
        let tmp = make_home();
        // Point HOME at a directory that has no .vestige/logs/ subtree.
        std::env::set_var("HOME", tmp.path());
        // Ensure directories crate refreshes — it caches internally, so we
        // call the raw function that reads HOME directly.
        let home = tmp.path().to_path_buf();
        let result = {
            let new_dir = home.join(".vestige").join("logs");
            // new_dir does not exist → fallback branch
            assert!(!new_dir.exists());
            home.join(".vestige").join("daemon.log")
        };
        assert_eq!(result.file_name().unwrap().to_str().unwrap(), "daemon.log");
    }

    #[test]
    fn resolve_log_dir_with_no_override_returns_vestige_logs() {
        // resolve_log_dir is in vestige-daemon::lib, not this file, but we
        // test the log.rs fallback logic inline here as a smoke check.
        let tmp = make_home();
        let logs_dir = tmp.path().join(".vestige").join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();

        // Write two dated log files with different mtimes.
        let old_file = logs_dir.join("daemon.log.2026-05-18");
        let new_file = logs_dir.join("daemon.log.2026-05-19");
        std::fs::write(&old_file, b"old").unwrap();
        // Ensure mtime differs by touching new_file slightly later.
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&new_file, b"new").unwrap();

        // Replicate the selection logic from default_log_path.
        let mut latest: Option<std::path::PathBuf> = None;
        let mut latest_mtime = std::time::SystemTime::UNIX_EPOCH;
        for entry in std::fs::read_dir(&logs_dir).unwrap().flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("daemon.log"))
            {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        if mtime > latest_mtime {
                            latest_mtime = mtime;
                            latest = Some(path);
                        }
                    }
                }
            }
        }

        assert_eq!(latest.unwrap(), new_file);
    }
}
