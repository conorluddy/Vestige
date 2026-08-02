//! `vestige daemon install` — render and install the LaunchAgent plist,
//! then `launchctl load -w`.
//!
//! The plist template is embedded at compile time. All rendering logic lives
//! in `vestige_daemon::plist`; this file is a thin CLI adapter.

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use std::path::PathBuf;
use std::process::Command;
use vestige_daemon::plist::{default_plist_path, render, LAUNCH_AGENT_LABEL};

// === CONSTANTS ===

const PLIST_TEMPLATE: &str = include_str!("../../../templates/com.vestige.daemon.plist.tmpl");

// === TYPES ===

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Overwrite an existing plist without prompting.
    #[arg(long)]
    pub force: bool,

    /// Skip the `launchctl load -w` step. Useful for dry-runs or test harnesses.
    #[arg(long)]
    pub no_load: bool,

    /// Override the path to the vestige binary that the plist invokes.
    /// Default: result of `std::env::current_exe()` (canonicalised).
    #[arg(long, value_name = "PATH")]
    pub bin: Option<PathBuf>,

    /// JSON output for scripts.
    #[arg(long)]
    pub json: bool,

    /// Suppress the macOS menu-bar app boot prompt (V0.5.2). Always implied in
    /// agent / CI / non-TTY contexts; this flag opts out explicitly.
    #[arg(long)]
    pub no_ui: bool,

    /// Accept the menu-bar app boot prompt non-interactively (for scripted human setup).
    #[arg(long)]
    pub yes: bool,
}

// === PUBLIC API ===

pub fn run(args: InstallArgs) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!(
            "vestige daemon install is macOS-only (LaunchAgents). \
             Linux systemd support lands in V0.6."
        );
    }

    let bin = resolve_bin(args.bin)?;
    let home = home_dir().ok_or_else(|| anyhow!("HOME is not set"))?;
    let rendered = render(PLIST_TEMPLATE, &bin, &home);

    let plist_path = default_plist_path();
    if plist_path.exists() && !args.force {
        bail!(
            "plist already exists at {} — re-run with --force to overwrite \
             (uninstall first if it is currently loaded)",
            plist_path.display()
        );
    }

    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).context("create ~/Library/LaunchAgents")?;
    }
    std::fs::write(&plist_path, &rendered)
        .with_context(|| format!("write plist to {}", plist_path.display()))?;

    lint_plist_silent(&plist_path);

    let loaded = if args.no_load {
        false
    } else {
        launchctl_load(&plist_path)?;
        true
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "installed": true,
                "plist_path": plist_path.display().to_string(),
                "loaded": loaded,
                "vestige_bin": bin.display().to_string(),
                "label": LAUNCH_AGENT_LABEL,
            }))?
        );
    } else {
        println!("daemon installed");
        println!("  plist:    {}", plist_path.display());
        println!("  vestige:  {}", bin.display());
        if loaded {
            println!("  launchctl: loaded");
        } else {
            println!("  launchctl: skipped (--no-load)");
        }
    }

    // V0.5.2: offer to boot the menu-bar app (gated to interactive macOS; see
    // commands::ui::decide_boot). Suppressed in JSON / agent / CI runs.
    if !args.json {
        crate::commands::ui::maybe_offer_boot(args.no_ui, args.yes);
    }

    Ok(())
}

// === PRIVATE HELPERS ===

/// Resolve the absolute path to the vestige binary.
///
/// Uses `--bin` when supplied, otherwise falls back to the running executable.
fn resolve_bin(bin_override: Option<PathBuf>) -> Result<PathBuf> {
    match bin_override {
        Some(p) => p
            .canonicalize()
            .context("--bin path does not exist or is not accessible"),
        None => std::env::current_exe()
            .context("could not determine current executable path")?
            .canonicalize()
            .context("could not canonicalise current executable path"),
    }
}

/// Run `plutil -lint` on the written plist. Logs a warning on failure but does
/// not abort — `plutil` may be absent in unusual environments.
fn lint_plist_silent(plist_path: &std::path::Path) {
    match Command::new("plutil").arg("-lint").arg(plist_path).output() {
        Ok(out) if !out.status.success() => {
            eprintln!(
                "warning: plutil -lint failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        _ => {}
    }
}

/// Invoke `launchctl load -w <plist>` and propagate non-zero exit as an error.
fn launchctl_load(plist_path: &std::path::Path) -> Result<()> {
    let status = Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(plist_path)
        .status()
        .context("could not run `launchctl load -w`")?;

    if !status.success() {
        bail!("`launchctl load -w` exited with status {status}");
    }
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
