//! `vestige daemon start` — run the daemon in the foreground (default) or detach.

use clap::Args;

// === TYPES ===

#[derive(Args, Debug)]
pub struct StartArgs {
    /// Run in foreground, attached to the controlling terminal. Default.
    /// Required under launchd (launchd manages the process lifetime).
    #[arg(long, conflicts_with = "detach")]
    pub foreground: bool,
    /// Double-fork to background. Mutually exclusive with --foreground.
    /// Not yet implemented — use `vestige daemon install` for autostart via launchd.
    #[arg(long)]
    pub detach: bool,
}

// === PUBLIC API ===

pub fn run(args: StartArgs) -> anyhow::Result<()> {
    if args.detach {
        anyhow::bail!(
            "--detach is not yet implemented; use `vestige daemon install` to autostart \
             via launchd, or run without --detach for a foreground daemon"
        );
    }

    let opts = vestige_daemon::DaemonOpts {
        foreground: !args.detach, // defaults to true
        pid_file: None,
        socket_path: None,
        status_file: None,
        log_file: None,
        projects_root: None,
    };

    // Build a tokio runtime — same pattern as commands/mcp.rs.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime
        .block_on(vestige_daemon::run(opts))
        .map_err(|e| anyhow::anyhow!("daemon exited with error: {e}"))?;
    Ok(())
}
