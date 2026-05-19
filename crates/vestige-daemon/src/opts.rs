//! Daemon command options parsed by the CLI and passed into `run`.

use std::path::PathBuf;

/// Options forwarded from `vestige daemon` CLI flags into [`crate::run`].
///
/// All path overrides default to `None`, which causes the daemon to resolve
/// standard locations under `~/.vestige/`. Override them in tests to keep
/// state isolated in a `tempfile::TempDir`.
#[derive(Debug, Clone)]
pub struct DaemonOpts {
    /// Run attached to the controlling terminal (no fork). Default for direct
    /// CLI use; required under launchd (launchd manages the process lifetime).
    pub foreground: bool,

    /// Override `~/.vestige/daemon.pid` for tests.
    pub pid_file: Option<PathBuf>,

    /// Override `~/.vestige/daemon.sock` for tests.
    pub socket_path: Option<PathBuf>,

    /// Override `~/.vestige/daemon.status.json` for tests.
    pub status_file: Option<PathBuf>,

    /// Override `~/.vestige/daemon.log` for tests.
    pub log_file: Option<PathBuf>,

    /// Override `~/.vestige/projects/` for tests.
    ///
    /// When `None`, the daemon discovers project DBs from the canonical location.
    /// Tests should supply a `TempDir`-backed path so they don't inherit the
    /// caller's real project workers, which could have locks or WAL contention.
    pub projects_root: Option<PathBuf>,

    /// Override the resolved daemon config (cadences etc.) instead of reading from disk.
    ///
    /// Test-only escape hatch — production paths always pass `None`. When `Some`,
    /// `run_with_cancel` skips the `daemon_config_for` call and uses this config
    /// directly, allowing tests to set very short cadences (e.g. 2 s embed sweep)
    /// to prove the scheduler's tokio interval timers actually fire.
    pub config_override: Option<vestige_config::ResolvedDaemonConfig>,
}

impl Default for DaemonOpts {
    fn default() -> Self {
        Self {
            foreground: true,
            pid_file: None,
            socket_path: None,
            status_file: None,
            log_file: None,
            projects_root: None,
            config_override: None,
        }
    }
}
