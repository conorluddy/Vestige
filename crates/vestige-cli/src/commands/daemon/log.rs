//! `vestige daemon log [-f]` — print or tail the daemon log file.

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

    // Delegate to `tail` for portability and log-rotation robustness (`tail -F`).
    // V0.5 targets macOS only, so the BSD `tail` is always available.
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

fn default_log_path() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige").join("daemon.log"))
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".vestige")
                .join("daemon.log")
        })
}
