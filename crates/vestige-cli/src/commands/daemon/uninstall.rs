//! `vestige daemon uninstall` — `launchctl unload -w` then remove the plist.
//!
//! Inverse of `vestige daemon install`. Safe to call when the daemon is not
//! loaded — `launchctl unload` on an absent service is a no-op.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::process::Command;
use vestige_daemon::plist::{default_plist_path, LAUNCH_AGENT_LABEL};

// === TYPES ===

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Skip the `launchctl unload` step (just remove the plist file).
    #[arg(long)]
    pub no_unload: bool,

    /// Exit 0 if the plist is already absent rather than returning an error.
    #[arg(long)]
    pub if_exists: bool,

    /// JSON output for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: UninstallArgs) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("vestige daemon uninstall is macOS-only");
    }

    let plist_path = default_plist_path();
    let plist_present = plist_path.exists();

    if !plist_present && !args.if_exists {
        bail!("no plist found at {}", plist_path.display());
    }

    let unloaded = if !args.no_unload && plist_present {
        launchctl_unload(&plist_path)
    } else {
        false
    };

    let removed = if plist_present {
        std::fs::remove_file(&plist_path)
            .with_context(|| format!("remove {}", plist_path.display()))?;
        true
    } else {
        false
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "uninstalled": removed,
                "plist_path": plist_path.display().to_string(),
                "unloaded": unloaded,
                "label": LAUNCH_AGENT_LABEL,
            }))?
        );
    } else if removed {
        println!("daemon uninstalled");
        println!("  plist:    {} (removed)", plist_path.display());
        println!(
            "  launchctl: {}",
            if unloaded {
                "unloaded"
            } else {
                "skipped/raced"
            }
        );
    } else {
        println!("daemon plist not present");
    }

    Ok(())
}

// === PRIVATE HELPERS ===

/// Invoke `launchctl unload -w <plist>`.
///
/// Returns `true` if the command succeeds. Non-zero exit is treated as a
/// warning (logs to stderr) and returns `false` — the plist may have already
/// been unloaded or the service may have been registered under a different
/// session. We continue to remove the plist regardless.
fn launchctl_unload(plist_path: &std::path::Path) -> bool {
    match Command::new("launchctl")
        .arg("unload")
        .arg("-w")
        .arg(plist_path)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("warning: launchctl unload exited with status {status}");
            false
        }
        Err(e) => {
            eprintln!("warning: could not run `launchctl unload -w`: {e}");
            false
        }
    }
}
