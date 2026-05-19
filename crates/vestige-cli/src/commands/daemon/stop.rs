//! `vestige daemon stop` — read the pidfile, send SIGTERM, wait for exit.
//!
//! If the LaunchAgent (`com.vestige.daemon`) is currently loaded, a warning is
//! emitted after SIGTERM: launchd's `KeepAlive=true` will restart the daemon.
//! The JSON output includes `launchctl_managed: bool` so callers can act on it.
//! To permanently stop the daemon, run `vestige daemon uninstall`.

use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;
use vestige_daemon::plist::LAUNCH_AGENT_LABEL;

// === TYPES ===

#[derive(Args, Debug)]
pub struct StopArgs {
    /// Max seconds to wait for the daemon to exit before reporting failure.
    #[arg(long, default_value_t = 10)]
    pub timeout: u64,
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: StopArgs) -> anyhow::Result<()> {
    let launchctl_managed = is_launchagent_loaded(LAUNCH_AGENT_LABEL);

    // Resolve pidfile path using the public API on DaemonLifecycle.
    let pid_path = vestige_daemon::DaemonLifecycle::resolve_pid_path(None);

    // Read the PID. A missing pidfile means the daemon is not running — idempotent stop.
    let pid = match std::fs::read_to_string(&pid_path) {
        Ok(s) => s
            .trim()
            .parse::<u32>()
            .context("malformed pidfile: expected a numeric PID")
            .map(|p| p as i32)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            print_not_running(launchctl_managed, args.json);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    // Send SIGTERM. PIDs on macOS fit in i32 (max ~100 000 active processes); safe cast.
    // SAFETY: kill(2) is a POSIX syscall; valid pid and signal values are defined.
    let send_result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if send_result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            // Process already gone — treat as success (idempotent).
            print_stopped(pid, launchctl_managed, args.json);
            return Ok(());
        }
        anyhow::bail!("failed to signal pid {pid}: {err}");
    }

    // Poll every 100 ms until the process exits or timeout expires.
    let deadline = Instant::now() + Duration::from_secs(args.timeout);
    let mut exited = false;
    while Instant::now() < deadline {
        // kill(pid, 0) probes existence without sending a signal.
        // SAFETY: same POSIX call; signal 0 is always valid.
        let still_alive = unsafe { libc::kill(pid, 0) } == 0;
        if !still_alive {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if !exited {
        anyhow::bail!(
            "daemon (pid {pid}) did not exit within {}s after SIGTERM",
            args.timeout
        );
    }

    print_stopped(pid, launchctl_managed, args.json);
    Ok(())
}

// === PRIVATE HELPERS ===

/// Returns `true` if the named LaunchAgent label appears in `launchctl list` output.
///
/// Matches by checking whether any output line ends with the label string, which
/// is the format launchctl uses: `<pid-or-dash>  <exit-code>  <label>`.
/// Returns `false` on any error (launchctl absent, non-zero exit, non-UTF-8 output).
fn is_launchagent_loaded(label: &str) -> bool {
    let out = std::process::Command::new("launchctl").arg("list").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.ends_with(label)),
        _ => false,
    }
}

fn print_not_running(launchctl_managed: bool, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "running": false,
                "stopped": false,
                "launchctl_managed": launchctl_managed,
            })
        );
    } else {
        println!("daemon: not running");
    }
}

fn print_stopped(pid: i32, launchctl_managed: bool, json: bool) {
    if json {
        let mut obj = serde_json::json!({
            "running": false,
            "stopped": true,
            "pid": pid,
            "launchctl_managed": launchctl_managed,
        });
        if launchctl_managed {
            obj["note"] =
                serde_json::Value::String(
                    "launchd KeepAlive will restart the daemon; run `vestige daemon uninstall` to fully stop"
                        .to_string(),
                );
        }
        println!("{obj}");
    } else if launchctl_managed {
        println!("daemon stopped pid={pid} (SIGTERM sent cleanly)");
        eprintln!(
            "note: launchd KeepAlive will restart the daemon. \
             run `vestige daemon uninstall` to fully stop and remove the LaunchAgent."
        );
    } else {
        println!("daemon stopped pid={pid}");
    }
}
