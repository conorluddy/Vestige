//! `vestige daemon restart` — bounce the daemon. Under launchd, uses
//! `launchctl kickstart -k`. Otherwise emits a clear error directing
//! the user to `daemon install` (the supported autostart path).
//!
//! `launchctl kickstart -k gui/$UID/com.vestige.daemon` is the launchd-correct
//! way to restart a loaded KeepAlive agent: it terminates the running process
//! and launchd immediately respawns it. This avoids the `launchctl unload`/
//! `launchctl load` dance that can leave the agent in a disabled-override state.

use anyhow::{bail, Context};
use clap::Args;
use vestige_daemon::plist::LAUNCH_AGENT_LABEL;

// === TYPES ===

#[derive(Args, Debug)]
pub struct RestartArgs {
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: RestartArgs) -> anyhow::Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("vestige daemon restart is macOS-only (LaunchAgent dependency)");
    }

    if !is_launchagent_loaded(LAUNCH_AGENT_LABEL) {
        bail!(
            "daemon is not loaded via launchd; install with `vestige daemon install` first.\n\
             If you started it manually with `daemon start`, use `daemon stop` then `daemon start` to bounce."
        );
    }

    // SAFETY: getuid(2) is always safe — no pointers, no invariants to uphold.
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");

    let status = std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &target])
        .status()
        .context("could not run `launchctl kickstart -k`")?;

    if !status.success() {
        bail!("`launchctl kickstart -k {target}` exited with status {status}");
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "restarted": true,
                "launchctl_managed": true,
                "label": LAUNCH_AGENT_LABEL,
                "target": target,
            }))?
        );
    } else {
        println!("daemon restarted via launchctl kickstart -k");
        println!("  target: {target}");
    }

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
