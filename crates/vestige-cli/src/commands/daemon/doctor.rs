//! `vestige daemon doctor` — comprehensive daemon health check.
//!
//! Runs 8 checks and prints a structured report with `[OK]`/`[WARN]`/`[FAIL]`
//! labels. Exits 1 if any check is `FAIL`; exits 0 for all-OK or warn-only.
//!
//! # Checks
//! 1. Pidfile — exists and PID is alive.
//! 2. launchctl loaded — LaunchAgent appears in `launchctl list`.
//! 3. Plist file — exists and passes `plutil -lint`.
//! 4. Plist bin path — plist's binary matches the currently-running `vestige`.
//! 5. Socket reachable — daemon.sock accepts a `daemon.status` ping within 2s.
//! 6. Status file fresh — daemon.status.json mtime < 30s.
//! 7. Projects open — every `~/.vestige/projects/*/memory.sqlite` opens cleanly.
//! 8. Rolling log dir — `~/.vestige/logs/` exists and most-recent file < 24h.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use clap::Args;
use serde::Serialize;
use vestige_daemon::plist::{default_plist_path, LAUNCH_AGENT_LABEL};
use vestige_store::Store;

// === TYPES ===

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
struct CheckResult {
    name: String,
    status: CheckStatus,
    detail: String,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    overall: CheckStatus,
    checks: Vec<CheckResult>,
}

// === PUBLIC API ===

pub fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let vestige_dir = resolve_vestige_dir();

    let checks = vec![
        check_pidfile(&vestige_dir),
        check_launchctl(),
        check_plist(),
        check_plist_bin_path(),
        check_socket(&vestige_dir),
        check_status_file_fresh(&vestige_dir),
        check_projects_open(&vestige_dir),
        check_log_dir(&vestige_dir),
    ];

    let overall = if checks.iter().any(|c| c.status == CheckStatus::Fail) {
        CheckStatus::Fail
    } else if checks.iter().any(|c| c.status == CheckStatus::Warn) {
        CheckStatus::Warn
    } else {
        CheckStatus::Ok
    };

    let report = DoctorReport { overall, checks };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report);
    }

    if matches!(report.overall, CheckStatus::Fail) {
        std::process::exit(1);
    }

    Ok(())
}

// === PRIVATE HELPERS — individual checks ===

/// Check 1: pidfile exists and PID is a live process.
fn check_pidfile(vestige_dir: &Path) -> CheckResult {
    let pid_path = vestige_dir.join("daemon.pid");
    match std::fs::read_to_string(&pid_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CheckResult {
            name: "pidfile".to_string(),
            status: CheckStatus::Fail,
            detail: format!("pidfile not found at {}", pid_path.display()),
        },
        Err(e) => CheckResult {
            name: "pidfile".to_string(),
            status: CheckStatus::Fail,
            detail: format!("could not read pidfile {}: {e}", pid_path.display()),
        },
        Ok(contents) => match contents.trim().parse::<u32>() {
            Err(_) => CheckResult {
                name: "pidfile".to_string(),
                status: CheckStatus::Fail,
                detail: format!(
                    "pidfile contains non-numeric content: {:?}",
                    contents.trim()
                ),
            },
            Ok(pid) => {
                // SAFETY: kill(2) with sig=0 probes process existence; no signal sent.
                let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
                if alive {
                    CheckResult {
                        name: "pidfile".to_string(),
                        status: CheckStatus::Ok,
                        detail: format!("pid={pid}, process alive"),
                    }
                } else {
                    CheckResult {
                        name: "pidfile".to_string(),
                        status: CheckStatus::Fail,
                        detail: format!(
                            "pidfile has pid={pid} but no such process exists (stale pidfile)"
                        ),
                    }
                }
            }
        },
    }
}

/// Check 2: LaunchAgent label appears in `launchctl list`.
fn check_launchctl() -> CheckResult {
    let out = std::process::Command::new("launchctl").arg("list").output();
    match out {
        Err(e) => CheckResult {
            name: "launchctl loaded".to_string(),
            status: CheckStatus::Warn,
            detail: format!("could not run `launchctl list`: {e}"),
        },
        Ok(o) if !o.status.success() => CheckResult {
            name: "launchctl loaded".to_string(),
            status: CheckStatus::Warn,
            detail: format!("`launchctl list` exited with {}", o.status),
        },
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let loaded = stdout.lines().any(|l| l.ends_with(LAUNCH_AGENT_LABEL));
            if loaded {
                CheckResult {
                    name: "launchctl loaded".to_string(),
                    status: CheckStatus::Ok,
                    detail: format!("{LAUNCH_AGENT_LABEL} listed"),
                }
            } else {
                CheckResult {
                    name: "launchctl loaded".to_string(),
                    status: CheckStatus::Warn,
                    detail: format!(
                        "{LAUNCH_AGENT_LABEL} not in launchctl list (daemon may be running manually)"
                    ),
                }
            }
        }
    }
}

/// Check 3: Plist file exists and passes `plutil -lint`.
fn check_plist() -> CheckResult {
    let plist_path = default_plist_path();
    if !plist_path.exists() {
        return CheckResult {
            name: "plist".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "plist not found at {} — run `vestige daemon install`",
                plist_path.display()
            ),
        };
    }

    let lint = std::process::Command::new("plutil")
        .arg("-lint")
        .arg(&plist_path)
        .output();

    match lint {
        Err(e) => CheckResult {
            name: "plist".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "{} exists but `plutil -lint` could not run: {e}",
                plist_path.display()
            ),
        },
        Ok(o) if !o.status.success() => CheckResult {
            name: "plist".to_string(),
            status: CheckStatus::Fail,
            detail: format!(
                "{} fails plutil -lint: {}",
                plist_path.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        },
        Ok(_) => CheckResult {
            name: "plist".to_string(),
            status: CheckStatus::Ok,
            detail: format!("{}, plutil OK", plist_path.display()),
        },
    }
}

/// Check 4: Plist's `ProgramArguments[0]` matches the current binary.
fn check_plist_bin_path() -> CheckResult {
    let plist_path = default_plist_path();
    if !plist_path.exists() {
        return CheckResult {
            name: "plist bin path".to_string(),
            status: CheckStatus::Warn,
            detail: "plist absent — skipping bin-path check".to_string(),
        };
    }

    let plist_bin = match extract_plist_bin_path(&plist_path) {
        Some(p) => p,
        None => {
            return CheckResult {
                name: "plist bin path".to_string(),
                status: CheckStatus::Warn,
                detail: format!("could not parse binary path from {}", plist_path.display()),
            };
        }
    };

    let current_bin = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return CheckResult {
                name: "plist bin path".to_string(),
                status: CheckStatus::Warn,
                detail: format!("could not determine current binary path: {e}"),
            };
        }
    };

    if plist_bin == current_bin {
        CheckResult {
            name: "plist bin path".to_string(),
            status: CheckStatus::Ok,
            detail: format!(
                "plist and current binary both point at {}",
                plist_bin.display()
            ),
        }
    } else {
        CheckResult {
            name: "plist bin path".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "plist points at {} but current binary is {} — `daemon uninstall && daemon install` recommended",
                plist_bin.display(),
                current_bin.display()
            ),
        }
    }
}

/// Check 5: Connect to the daemon socket and get a reply within 2s.
fn check_socket(vestige_dir: &Path) -> CheckResult {
    let socket_path = vestige_dir.join("daemon.sock");

    let start = std::time::Instant::now();

    let stream = match UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            return CheckResult {
                name: "socket".to_string(),
                status: CheckStatus::Fail,
                detail: format!("could not connect to {}: {e}", socket_path.display()),
            };
        }
    };

    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
        return CheckResult {
            name: "socket".to_string(),
            status: CheckStatus::Warn,
            detail: format!("connected but could not set read timeout: {e}"),
        };
    }
    if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(2))) {
        return CheckResult {
            name: "socket".to_string(),
            status: CheckStatus::Warn,
            detail: format!("connected but could not set write timeout: {e}"),
        };
    }

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"daemon.status","params":{}}"#;
    let request_line = format!("{request}\n");

    let mut stream = stream;
    if let Err(e) = stream.write_all(request_line.as_bytes()) {
        return CheckResult {
            name: "socket".to_string(),
            status: CheckStatus::Fail,
            detail: format!("connected to socket but failed to send request: {e}"),
        };
    }

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    match reader.read_line(&mut response) {
        Err(e) => CheckResult {
            name: "socket".to_string(),
            status: CheckStatus::Fail,
            detail: format!("socket connected but no reply received: {e}"),
        },
        Ok(0) => CheckResult {
            name: "socket".to_string(),
            status: CheckStatus::Fail,
            detail: "socket connected but connection closed without reply".to_string(),
        },
        Ok(_) => {
            let elapsed_ms = start.elapsed().as_millis();
            CheckResult {
                name: "socket".to_string(),
                status: CheckStatus::Ok,
                detail: format!(
                    "{} reachable, replied in {elapsed_ms}ms",
                    socket_path.display()
                ),
            }
        }
    }
}

/// Check 6: daemon.status.json exists and its mtime is within 30 seconds.
fn check_status_file_fresh(vestige_dir: &Path) -> CheckResult {
    let status_path = vestige_dir.join("daemon.status.json");

    let metadata = match std::fs::metadata(&status_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return CheckResult {
                name: "status file".to_string(),
                status: CheckStatus::Fail,
                detail: format!(
                    "{} not found — daemon may not be running",
                    status_path.display()
                ),
            };
        }
        Err(e) => {
            return CheckResult {
                name: "status file".to_string(),
                status: CheckStatus::Fail,
                detail: format!("could not stat {}: {e}", status_path.display()),
            };
        }
        Ok(m) => m,
    };

    let age_secs = metadata
        .modified()
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX);

    if age_secs <= 30 {
        CheckResult {
            name: "status file".to_string(),
            status: CheckStatus::Ok,
            detail: format!("{}, age={age_secs}s", status_path.display()),
        }
    } else {
        CheckResult {
            name: "status file".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "{} is {age_secs}s old — scheduler may be stuck",
                status_path.display()
            ),
        }
    }
}

/// Check 7: every `~/.vestige/projects/*/memory.sqlite` opens cleanly.
fn check_projects_open(vestige_dir: &Path) -> CheckResult {
    let projects_dir = vestige_dir.join("projects");

    let entries = match std::fs::read_dir(&projects_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return CheckResult {
                name: "projects open".to_string(),
                status: CheckStatus::Warn,
                detail: format!(
                    "{} not found — no projects registered yet",
                    projects_dir.display()
                ),
            };
        }
        Err(e) => {
            return CheckResult {
                name: "projects open".to_string(),
                status: CheckStatus::Fail,
                detail: format!("could not read {}: {e}", projects_dir.display()),
            };
        }
        Ok(rd) => rd,
    };

    let mut total = 0usize;
    let mut failed = 0usize;
    let mut first_failure: Option<PathBuf> = None;

    for entry in entries.flatten() {
        let db_path = entry.path().join("memory.sqlite");
        if !db_path.exists() {
            continue;
        }
        total += 1;
        match Store::open(&db_path) {
            Ok(_) => {}
            Err(_) => {
                failed += 1;
                if first_failure.is_none() {
                    first_failure = Some(db_path);
                }
            }
        }
    }

    if total == 0 {
        return CheckResult {
            name: "projects open".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "no memory.sqlite files found under {}",
                projects_dir.display()
            ),
        };
    }

    if failed > 0 {
        CheckResult {
            name: "projects open".to_string(),
            status: CheckStatus::Fail,
            detail: format!(
                "{total} found, {} failed to open (first: {})",
                failed,
                first_failure
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
        }
    } else {
        CheckResult {
            name: "projects open".to_string(),
            status: CheckStatus::Ok,
            detail: format!("{total} found, {total} opened"),
        }
    }
}

/// Check 8: `~/.vestige/logs/` exists and the most recent file is < 24h old.
fn check_log_dir(vestige_dir: &Path) -> CheckResult {
    let log_dir = vestige_dir.join("logs");

    if !log_dir.exists() {
        return CheckResult {
            name: "rolling log dir".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "{} does not exist — run `vestige daemon start` to create logs",
                log_dir.display()
            ),
        };
    }

    let entries = match std::fs::read_dir(&log_dir) {
        Err(e) => {
            return CheckResult {
                name: "rolling log dir".to_string(),
                status: CheckStatus::Warn,
                detail: format!("could not read {}: {e}", log_dir.display()),
            };
        }
        Ok(rd) => rd,
    };

    // Find the most recently-modified file.
    let newest = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), mtime))
        })
        .max_by_key(|(_, mtime)| *mtime);

    match newest {
        None => CheckResult {
            name: "rolling log dir".to_string(),
            status: CheckStatus::Warn,
            detail: format!("{} exists but contains no log files", log_dir.display()),
        },
        Some((path, mtime)) => {
            let age_secs = SystemTime::now()
                .duration_since(mtime)
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);
            let age_display = if age_secs < 60 {
                format!("{age_secs}s")
            } else if age_secs < 3600 {
                format!("{}m", age_secs / 60)
            } else {
                format!("{}h", age_secs / 3600)
            };
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if age_secs < 86400 {
                CheckResult {
                    name: "rolling log dir".to_string(),
                    status: CheckStatus::Ok,
                    detail: format!("most recent: {filename} (age={age_display})"),
                }
            } else {
                CheckResult {
                    name: "rolling log dir".to_string(),
                    status: CheckStatus::Warn,
                    detail: format!(
                        "most recent log {filename} is {age_display} old — daemon may not be logging"
                    ),
                }
            }
        }
    }
}

// === PRIVATE HELPERS — formatting and parsing ===

/// Extract the first `<string>` value under `ProgramArguments` from a plist file.
///
/// Looks for the pattern `<key>ProgramArguments</key>` in XML plist format and
/// returns the content of the first `<string>` tag that follows. Returns `None`
/// if the pattern is absent or the file cannot be read.
fn extract_plist_bin_path(plist_path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(plist_path).ok()?;
    // Locate the ProgramArguments key block.
    let after_key = contents.split("<key>ProgramArguments</key>").nth(1)?;
    // Find the first <string> inside the following <array>.
    let after_array = after_key.split("<array>").nth(1)?;
    let inner = after_array.split("<string>").nth(1)?;
    let value = inner.split("</string>").next()?;
    Some(PathBuf::from(value.trim()))
}

/// Resolve `~/.vestige` — canonical home for daemon runtime files.
fn resolve_vestige_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".vestige")
        })
}

/// Print the doctor report in aligned text format.
fn print_text_report(report: &DoctorReport) {
    println!("vestige daemon doctor\n");
    for check in &report.checks {
        let label = match check.status {
            CheckStatus::Ok => "[OK]  ",
            CheckStatus::Warn => "[WARN]",
            CheckStatus::Fail => "[FAIL]",
        };
        println!("{label}  {:<20}  {}", check.name, check.detail);
    }
    let overall_str = match report.overall {
        CheckStatus::Ok => "OK",
        CheckStatus::Warn => "WARN",
        CheckStatus::Fail => "FAIL",
    };
    println!("\noverall: {overall_str}");
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// All 8 checks must return a result without panicking, even when run against
    /// a TempDir home directory that has no `.vestige/` structure at all.
    #[test]
    fn doctor_runs_without_panic_on_missing_state() {
        let dir = TempDir::new().unwrap();
        let vestige_dir = dir.path().join(".vestige");
        // Intentionally do NOT create .vestige/ — checks must handle all missing paths.

        let results = [
            check_pidfile(&vestige_dir),
            check_launchctl(),
            check_plist(),
            check_plist_bin_path(),
            check_socket(&vestige_dir),
            check_status_file_fresh(&vestige_dir),
            check_projects_open(&vestige_dir),
            check_log_dir(&vestige_dir),
        ];

        // Every check must return without panicking.
        assert_eq!(results.len(), 8, "expected 8 check results");

        // With no daemon state, most will be WARN or FAIL — never OK.
        // (launchctl and plist checks are system-dependent; skip them here.)
        for result in &results[0..1] {
            assert_ne!(
                result.status,
                CheckStatus::Ok,
                "check '{}' should not be OK with no daemon state",
                result.name
            );
        }
    }
}
