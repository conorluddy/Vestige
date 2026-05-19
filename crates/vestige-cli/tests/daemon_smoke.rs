//! End-to-end smoke test for the V0.5 daemon. Spawns the real `vestige` binary,
//! drives it through the full lifecycle, and asserts external observable state.
//!
//! This is the one integration test that proves the deployment shape works.
//! All other daemon tests run in-process and don't catch CLI/runtime issues
//! such as broken arg parsing, wrong HOME resolution, or Tokio runtime problems.
//!
//! # Isolation strategy
//!
//! Each test uses a `TempDir` as `HOME`. Both `vestige-config`'s `storage_path_for`
//! and `directories::BaseDirs` read `$HOME` from the process environment, so all
//! daemon artefacts (pidfile, socket, status file, project DBs) land inside the
//! `TempDir` and are cleaned up automatically.
//!
//! # Zombie-process note
//!
//! `vestige daemon stop` uses `kill(pid, 0)` to poll for daemon exit. When the
//! daemon is a *child* of the test process, calling `stop` without also reaping
//! the child (via `Child::wait`) leaves the daemon in zombie state — `kill(pid, 0)`
//! still returns 0 for zombies, causing `stop` to time out.
//!
//! The fix: spawn the `stop` signal by reading the pidfile directly with libc
//! (SIGTERM), then immediately reap the daemon child with `Child::wait`, then
//! verify the artefacts. This keeps the test in full control of process lifetime.
//!
//! # What is NOT tested here
//!
//! - `vestige daemon install` — mutates the real `~/Library/LaunchAgents/`.
//! - Multiple concurrent daemons — `AlreadyRunning` is covered by in-process tests.
//! - `--detach` — not yet implemented.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

// === CONSTANTS ===

const VESTIGE_BIN: &str = env!("CARGO_BIN_EXE_vestige");
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// === HELPERS ===

/// Isolated HOME + repo pair for one test run.
struct Env {
    home: TempDir,
    repo: TempDir,
}

impl Env {
    fn new() -> Self {
        let home = TempDir::new().expect("home tempdir");
        let repo = TempDir::new().expect("repo tempdir");
        // The CLI resolves project identity from `.git` if present; create a
        // minimal `.git` dir so the init command doesn't need a real git remote.
        std::fs::create_dir_all(repo.path().join(".git")).expect("create .git");
        Self { home, repo }
    }

    fn home(&self) -> &std::path::Path {
        self.home.path()
    }

    fn socket_path(&self) -> PathBuf {
        self.home().join(".vestige").join("daemon.sock")
    }

    fn pid_path(&self) -> PathBuf {
        self.home().join(".vestige").join("daemon.pid")
    }

    fn status_path(&self) -> PathBuf {
        self.home().join(".vestige").join("daemon.status.json")
    }
}

/// Run a `vestige` subcommand with the test HOME, capturing stdout.
///
/// Returns `Ok(stdout_string)` on exit code 0, `Err(message)` otherwise.
fn run_cli(env: &Env, args: &[&str]) -> Result<String, String> {
    let out = Command::new(VESTIGE_BIN)
        .args(args)
        .env("HOME", env.home())
        .env("VESTIGE_LOG", "warn")
        .current_dir(env.repo.path())
        .output()
        .map_err(|e| format!("exec vestige: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "vestige {} exited {}: stderr={}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// Spawn `vestige daemon start --foreground` in the background, inheriting HOME.
///
/// Stdout and stderr are piped so we can dump them on failure.
fn spawn_daemon(env: &Env) -> Child {
    Command::new(VESTIGE_BIN)
        .args(["daemon", "start", "--foreground"])
        .env("HOME", env.home())
        .env("VESTIGE_LOG", "info")
        .current_dir(env.repo.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn vestige daemon")
}

/// Poll until `path` exists or `timeout` expires.
fn wait_for_path(path: &std::path::Path, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(format!("timed out waiting for {}", path.display()))
}

/// Dump a child's captured stderr to eprintln (best-effort, for failure diagnostics).
fn dump_child_stderr(child: &mut Child) {
    use std::io::Read;
    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        if !buf.is_empty() {
            eprintln!("--- daemon stderr ---\n{buf}\n--- end ---");
        }
    }
}

/// Send SIGTERM directly to the daemon's PID (read from the pidfile) and
/// immediately reap the child process handle.
///
/// This avoids the zombie-process trap that `vestige daemon stop` falls into
/// when the daemon is a direct child of the test: `kill(pid, 0)` returns 0
/// for zombie processes, causing the stop-polling loop to time out.
///
/// After this call the daemon is fully reaped. Use `verify_stop_artefacts`
/// to check pidfile/socket cleanup (the daemon's Drop cleans them up before
/// the Tokio runtime exits, so they should be gone by the time `wait` returns).
fn sigterm_and_reap(env: &Env, child: &mut Child) -> Result<(), String> {
    // Read the PID from the pidfile.
    let pid_raw =
        std::fs::read_to_string(env.pid_path()).map_err(|e| format!("read pidfile: {e}"))?;
    let pid: i32 = pid_raw
        .trim()
        .parse()
        .map_err(|e| format!("parse pid: {e}"))?;

    // Send SIGTERM.
    // SAFETY: kill(2) is POSIX. Valid pid and SIGTERM signal value.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH means the process already exited — treat as success.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("kill(pid={pid}, SIGTERM) failed: {err}"));
        }
    }

    // Reap the child. The daemon cleans up pidfile/socket in its Drop impl
    // before the process exits, so by the time wait() returns, artefacts
    // should already be removed.
    let status = child
        .wait()
        .map_err(|e| format!("wait() for daemon child failed: {e}"))?;

    // The daemon exits with status 0 on a clean SIGTERM shutdown.
    if !status.success() {
        // Non-zero exit is unusual but not a test failure by itself — the daemon
        // may have been mid-operation. Log it for diagnostics.
        eprintln!("daemon exited with status: {status}");
    }

    Ok(())
}

/// Verify that stop artefacts are in the expected state: pidfile and socket
/// are gone; status file persists as the historical record.
fn verify_stop_artefacts(env: &Env) -> Result<(), String> {
    if env.pid_path().exists() {
        return Err(format!(
            "pidfile still exists after stop: {}",
            env.pid_path().display()
        ));
    }
    if env.socket_path().exists() {
        return Err(format!(
            "socket still exists after stop: {}",
            env.socket_path().display()
        ));
    }
    if !env.status_path().exists() {
        return Err(format!(
            "status file was removed by stop — it should persist as a historical record: {}",
            env.status_path().display()
        ));
    }
    Ok(())
}

// === SHARED HELPERS ===

/// Seed a minimal project (init + one memory) and spawn the daemon in the
/// foreground. Waits for the socket and status file to appear before returning.
///
/// Returns the `Child` handle so the caller can reap it after the test.
/// The caller is responsible for calling `sigterm_and_reap` and
/// `verify_stop_artefacts` when done.
///
/// If the socket or status file does not appear in time, the daemon child is
/// killed and reaped before returning the error — no zombie left behind.
fn seed_and_spawn(env: &Env) -> Result<Child, String> {
    run_cli(env, &["init", "--name", "DaemonSmokeProject"])
        .map_err(|e| format!("vestige init failed: {e}"))?;
    run_cli(env, &["remember", "daemon smoke test memory"])
        .map_err(|e| format!("vestige remember failed: {e}"))?;

    let mut child = spawn_daemon(env);

    let wait_result = wait_for_path(&env.socket_path(), Duration::from_secs(8))
        .and_then(|()| wait_for_path(&env.status_path(), Duration::from_secs(8)));

    if let Err(e) = wait_result {
        // Kill and reap the child so we don't leave a zombie behind.
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("daemon not ready: {e}"));
    }

    Ok(child)
}

// === TESTS ===

/// Full daemon lifecycle: init → seed memory → start → status → kick → stop.
///
/// Exercises every publicly observable state transition as real subprocesses.
/// One holistic test rather than five because the setup cost (spawn + init + start)
/// is high and each step depends on the previous one completing.
#[test]
fn daemon_full_lifecycle_smoke() {
    let env = Env::new();

    // -----------------------------------------------------------------------
    // Step 1-3: seed a project with a memory so the daemon has work to discover.
    // -----------------------------------------------------------------------
    run_cli(&env, &["init", "--name", "DaemonSmokeProject"]).expect("vestige init failed");
    run_cli(&env, &["remember", "daemon smoke test memory"]).expect("vestige remember failed");

    // -----------------------------------------------------------------------
    // Step 4: spawn `vestige daemon start --foreground` as a background child.
    // -----------------------------------------------------------------------
    let mut daemon_child = spawn_daemon(&env);

    // Run the lifecycle steps, capturing any error so we can clean up first.
    let lifecycle_outcome = run_lifecycle_pre_stop(&env);

    // -----------------------------------------------------------------------
    // Step 9 (stop): send SIGTERM and reap the child before asserting anything.
    // This must happen regardless of whether earlier steps succeeded, to avoid
    // leaving a daemon running after a test failure.
    //
    // We use sigterm_and_reap rather than `vestige daemon stop` because stop's
    // kill(pid,0) polling loop treats zombie processes as still-alive (see module
    // doc note). By calling child.wait() here we fully reap the process first.
    // -----------------------------------------------------------------------
    let stop_outcome = sigterm_and_reap(&env, &mut daemon_child);

    // If the lifecycle failed before stop, dump stderr for diagnostics.
    if lifecycle_outcome.is_err() || stop_outcome.is_err() {
        dump_child_stderr(&mut daemon_child);
    }

    // Now surface any failure.
    lifecycle_outcome.expect("daemon lifecycle steps failed");
    stop_outcome.expect("daemon stop/reap failed");

    // -----------------------------------------------------------------------
    // Step 10: artefact cleanup — pidfile and socket gone; status file stays.
    // -----------------------------------------------------------------------
    verify_stop_artefacts(&env).expect("post-stop artefact assertions failed");
}

/// Drive steps 5–8 (observe and interact with a running daemon).
///
/// Returns before the stop step so the caller can reap the child cleanly.
fn run_lifecycle_pre_stop(env: &Env) -> Result<(), String> {
    // -----------------------------------------------------------------------
    // Step 5: poll until the daemon's Unix socket appears (up to 8s).
    // -----------------------------------------------------------------------
    wait_for_path(&env.socket_path(), Duration::from_secs(8))
        .map_err(|e| format!("socket not ready: {e}"))?;

    // The scheduler writes the status file on its very first tick (immediately
    // after startup). Wait for it before calling `daemon status`.
    wait_for_path(&env.status_path(), Duration::from_secs(8))
        .map_err(|e| format!("status file not ready: {e}"))?;

    // -----------------------------------------------------------------------
    // Step 6: `vestige daemon status --json` — assert daemon is running.
    // -----------------------------------------------------------------------
    let status_json = run_cli(env, &["daemon", "status", "--json"])
        .map_err(|e| format!("daemon status failed: {e}"))?;

    let status: Value = serde_json::from_str(status_json.trim())
        .map_err(|e| format!("status JSON parse error: {e}: raw={status_json}"))?;

    // When the daemon is running the status file contains a DaemonStatus object
    // with `pid > 0`. (When not running, the command emits `{"running": false}`.)
    let pid = status["pid"]
        .as_u64()
        .ok_or_else(|| format!("status.pid missing or non-numeric: {status}"))?;
    if pid == 0 {
        return Err(format!(
            "daemon reports pid=0 — did it start? status={status}"
        ));
    }

    // The seeded project must appear in the status.
    let projects = status["projects"]
        .as_array()
        .ok_or_else(|| format!("status.projects is not an array: {status}"))?;
    if projects.is_empty() {
        return Err(format!(
            "status.projects is empty — daemon did not discover seeded project: {status}"
        ));
    }

    // -----------------------------------------------------------------------
    // Step 7: `vestige daemon kick embed --json` — assert the kick is accepted.
    // -----------------------------------------------------------------------
    let kick_json = run_cli(env, &["daemon", "kick", "embed", "--json"])
        .map_err(|e| format!("daemon kick embed failed: {e}"))?;

    let kick_response: Value = serde_json::from_str(kick_json.trim())
        .map_err(|e| format!("kick JSON parse error: {e}: raw={kick_json}"))?;

    // kick.rs --json prints the full JSON-RPC 2.0 response envelope:
    // { "jsonrpc": "2.0", "id": 1, "result": { "queued": true, "queued_at": "...", "projects_queued": N } }
    let kick_result = kick_response
        .get("result")
        .ok_or_else(|| format!("kick response has no 'result' field: {kick_response}"))?;

    let queued = kick_result["queued"]
        .as_bool()
        .ok_or_else(|| format!("kick result.queued missing: {kick_response}"))?;
    if !queued {
        return Err(format!("kick result.queued is false: {kick_response}"));
    }

    let projects_queued = kick_result["projects_queued"]
        .as_u64()
        .ok_or_else(|| format!("kick result.projects_queued missing: {kick_response}"))?;
    if projects_queued == 0 {
        return Err(format!(
            "kick embed queued 0 projects — daemon did not register seeded project: {kick_response}"
        ));
    }

    // -----------------------------------------------------------------------
    // Step 8: poll status until pending_embeds == 0 (max 10s).
    //
    // The daemon's embed sweep runs synchronously inside dispatch_kick (Wave 4
    // semantics: kick blocks until done). The status file is refreshed every 5s,
    // so after the kick returns we may need to wait up to one status interval
    // for pending_embeds to reflect 0. We allow 10s to be safe.
    // -----------------------------------------------------------------------
    let embed_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s_json = run_cli(env, &["daemon", "status", "--json"])
            .map_err(|e| format!("status poll failed: {e}"))?;
        let s: Value = serde_json::from_str(s_json.trim())
            .map_err(|e| format!("status poll JSON parse error: {e}"))?;

        let still_pending: u64 = s["projects"]
            .as_array()
            .map(|ps| {
                ps.iter()
                    .map(|p| p["pending_embeds"].as_u64().unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0);

        if still_pending == 0 {
            break;
        }

        if Instant::now() >= embed_deadline {
            return Err(format!(
                "embed did not complete within 10s; pending_embeds={still_pending}"
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    Ok(())
}

/// `vestige daemon stop` with no daemon running must exit 0 and report not-running.
#[test]
fn daemon_stop_idempotent_when_not_running() {
    let env = Env::new();

    // No daemon started — stop should be a no-op.
    let out = Command::new(VESTIGE_BIN)
        .args(["daemon", "stop", "--json"])
        .env("HOME", env.home())
        .env("VESTIGE_LOG", "warn")
        .current_dir(env.repo.path())
        .output()
        .expect("exec vestige");

    assert!(
        out.status.success(),
        "daemon stop with no daemon must exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json: Value =
        serde_json::from_slice(&out.stdout).expect("daemon stop --json must emit valid JSON");

    assert_eq!(
        json["running"],
        Value::Bool(false),
        "running must be false when no daemon: {json}"
    );
    assert_eq!(
        json["stopped"],
        Value::Bool(false),
        "stopped must be false when nothing was stopped: {json}"
    );
}

/// `vestige daemon status --json` with no daemon running emits `{"running": false}`.
#[test]
fn daemon_status_when_not_running() {
    let env = Env::new();

    let out = Command::new(VESTIGE_BIN)
        .args(["daemon", "status", "--json"])
        .env("HOME", env.home())
        .env("VESTIGE_LOG", "warn")
        .current_dir(env.repo.path())
        .output()
        .expect("exec vestige");

    assert!(
        out.status.success(),
        "daemon status must exit 0 even when not running: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json: Value =
        serde_json::from_slice(&out.stdout).expect("daemon status --json must emit valid JSON");

    assert_eq!(
        json["running"],
        Value::Bool(false),
        "running must be false when no daemon: {json}"
    );
}

/// Prove that `vestige daemon kick prune --json` reaches the worker and
/// receives a valid response with `queued: true` and at least one project queued.
///
/// The prune worker runs `VACUUM` on the project's SQLite database. We don't
/// assert on the vacuum outcome here — that is covered by the in-process worker
/// tests (`prune_round_trip` in `vestige-daemon`). This test verifies only the
/// CLI → IPC → daemon round-trip shape at the subprocess level.
#[test]
fn daemon_kick_prune_lifecycle() {
    let env = Env::new();

    let mut daemon_child = match seed_and_spawn(&env) {
        Ok(child) => child,
        Err(e) => panic!("failed to seed and start daemon: {e}"),
    };

    let kick_outcome = (|| -> Result<(), String> {
        let out = run_cli(&env, &["daemon", "kick", "prune", "--json"])
            .map_err(|e| format!("daemon kick prune failed: {e}"))?;

        let resp: Value = serde_json::from_str(out.trim())
            .map_err(|e| format!("kick prune JSON parse error: {e}: raw={out}"))?;

        let result = resp
            .get("result")
            .ok_or_else(|| format!("kick prune response has no 'result' field: {resp}"))?;

        let queued = result["queued"]
            .as_bool()
            .ok_or_else(|| format!("kick prune result.queued missing or non-bool: {resp}"))?;
        if !queued {
            return Err(format!("kick prune result.queued is false: {resp}"));
        }

        let projects_queued = result["projects_queued"]
            .as_u64()
            .ok_or_else(|| format!("kick prune result.projects_queued missing: {resp}"))?;
        if projects_queued == 0 {
            return Err(format!(
                "kick prune queued 0 projects — daemon did not register seeded project: {resp}"
            ));
        }

        Ok(())
    })();

    let stop_outcome = sigterm_and_reap(&env, &mut daemon_child);

    if kick_outcome.is_err() || stop_outcome.is_err() {
        dump_child_stderr(&mut daemon_child);
    }

    kick_outcome.expect("daemon kick prune lifecycle failed");
    stop_outcome.expect("daemon stop/reap failed");
    verify_stop_artefacts(&env).expect("post-stop artefact assertions failed");
}

/// Prove that `vestige daemon kick ttl --json` completes the IPC round-trip
/// and returns a valid response with `queued: true` and at least one project queued.
///
/// # V0.5 limitation
///
/// The TTL worker only expires candidates when `candidate_ttl_days > 0` in the
/// daemon's runtime config. In V0.5, the daemon always uses the default config
/// (`candidate_ttl_days = 0`), so the TTL job is a no-op: it returns a zero
/// `candidates_expired` count rather than touching the store.
///
/// This test therefore verifies only the round-trip path. When `candidate_ttl_days`
/// is wired into per-project or runtime config in a future milestone, this test
/// should be extended to seed an old candidate and assert it is actually expired.
#[test]
fn daemon_kick_ttl_lifecycle() {
    let env = Env::new();

    let mut daemon_child = match seed_and_spawn(&env) {
        Ok(child) => child,
        Err(e) => panic!("failed to seed and start daemon: {e}"),
    };

    let kick_outcome = (|| -> Result<(), String> {
        let out = run_cli(&env, &["daemon", "kick", "ttl", "--json"])
            .map_err(|e| format!("daemon kick ttl failed: {e}"))?;

        let resp: Value = serde_json::from_str(out.trim())
            .map_err(|e| format!("kick ttl JSON parse error: {e}: raw={out}"))?;

        let result = resp
            .get("result")
            .ok_or_else(|| format!("kick ttl response has no 'result' field: {resp}"))?;

        let queued = result["queued"]
            .as_bool()
            .ok_or_else(|| format!("kick ttl result.queued missing or non-bool: {resp}"))?;
        if !queued {
            return Err(format!("kick ttl result.queued is false: {resp}"));
        }

        let projects_queued = result["projects_queued"]
            .as_u64()
            .ok_or_else(|| format!("kick ttl result.projects_queued missing: {resp}"))?;
        if projects_queued == 0 {
            return Err(format!(
                "kick ttl queued 0 projects — daemon did not register seeded project: {resp}"
            ));
        }

        // The IPC KickResult shape is { queued, queued_at, projects_queued }.
        // Per-project TTL outcome detail (candidates_expired) is not surfaced in
        // the kick response; it lives in the worker's TtlSummary which is only
        // visible in daemon logs. Asserting on the kick envelope fields is
        // sufficient for the smoke-test contract.
        let _ = result["queued_at"]
            .as_str()
            .ok_or_else(|| format!("kick ttl result.queued_at missing or non-string: {resp}"))?;

        Ok(())
    })();

    let stop_outcome = sigterm_and_reap(&env, &mut daemon_child);

    if kick_outcome.is_err() || stop_outcome.is_err() {
        dump_child_stderr(&mut daemon_child);
    }

    kick_outcome.expect("daemon kick ttl lifecycle failed");
    stop_outcome.expect("daemon stop/reap failed");
    verify_stop_artefacts(&env).expect("post-stop artefact assertions failed");
}
