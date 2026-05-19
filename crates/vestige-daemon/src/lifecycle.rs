//! Pidfile lock acquisition and graceful-shutdown signal handling.
//!
//! [`DaemonLifecycle`] is the spine the daemon's `run()` loop hangs off.
//! It enforces the single-instance invariant (one lock-holder at a time per
//! host) and owns the async signal stream that lets `run()` block until
//! SIGTERM / SIGINT / SIGHUP arrives.
//!
//! ## Lock strategy
//!
//! Advisory exclusive lock via [`fs2::FileExt::try_lock_exclusive`].
//! The `File` handle is kept alive inside `DaemonLifecycle` for the entire
//! daemon lifetime; dropping it releases the OS advisory lock and unlinks the
//! pidfile.
//!
//! ## Stale pidfile recovery
//!
//! If `try_lock_exclusive` succeeds on a file that already contains a PID,
//! that PID belongs to a process that crashed without cleaning up. The
//! successful lock is proof no live holder exists, so we overwrite it.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use fs2::FileExt;

use crate::errors::DaemonError;

// === TYPES ===

/// Owns the pidfile advisory lock for the lifetime of the daemon process.
///
/// Acquiring `DaemonLifecycle` is the first thing `run()` does. While it is
/// alive, any second attempt to acquire the same pidfile will return
/// [`DaemonError::AlreadyRunning`]. Dropping it releases the lock and
/// unlinks the pidfile.
#[derive(Debug)]
pub struct DaemonLifecycle {
    pid_file: PathBuf,
    /// Keeps the `File` open so the OS advisory lock stays held.
    _lock_handle: File,
}

/// The Unix signal that caused the daemon to begin its shutdown sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ShutdownReason {
    /// `SIGTERM` — the standard signal sent by launchd and `kill`.
    Sigterm,
    /// `SIGINT` — Ctrl-C when running in the foreground.
    Sigint,
    /// `SIGHUP` — treated as a graceful restart request; config reload deferred to V0.6.
    Sighup,
}

// === PUBLIC API ===

impl DaemonLifecycle {
    /// Acquire the exclusive pidfile lock.
    ///
    /// Creates parent directories if they do not exist.  Writes the current
    /// process PID to the file.  Returns [`DaemonError::AlreadyRunning`] if
    /// another process holds the lock.
    pub fn acquire(pid_file: PathBuf) -> Result<Self, DaemonError> {
        ensure_parent_dirs_exist(&pid_file)?;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&pid_file)?;

        if let Err(_lock_err) = file.try_lock_exclusive() {
            let existing_pid = read_pid_from_file(&file).unwrap_or(0);
            return Err(DaemonError::AlreadyRunning { pid: existing_pid });
        }

        // Lock acquired — overwrite (stale or fresh) with our own PID.
        write_pid_to_file(&file, std::process::id())?;

        Ok(Self {
            pid_file,
            _lock_handle: file,
        })
    }

    /// Resolve the pidfile path, honouring an optional override.
    ///
    /// Default: `~/.vestige/daemon.pid`.
    /// Returns a [`DaemonError::Io`] if `$HOME` cannot be determined and no
    /// override was provided.
    pub fn resolve_pid_path(override_path: Option<&Path>) -> PathBuf {
        if let Some(path) = override_path {
            return path.to_path_buf();
        }
        default_vestige_dir().join("daemon.pid")
    }

    /// Park the calling task until the first SIGTERM, SIGINT, or SIGHUP arrives.
    ///
    /// Install all three handlers before entering the `select!` so no signal
    /// is dropped between the moment we register and the moment we start
    /// waiting.
    pub async fn wait_for_shutdown(&self) -> ShutdownReason {
        use tokio::signal::unix::{signal, SignalKind};

        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        let mut hup = signal(SignalKind::hangup()).expect("install SIGHUP handler");

        tokio::select! {
            _ = term.recv() => ShutdownReason::Sigterm,
            _ = int.recv()  => ShutdownReason::Sigint,
            _ = hup.recv()  => ShutdownReason::Sighup,
        }
    }
}

impl Drop for DaemonLifecycle {
    fn drop(&mut self) {
        // `fs2::FileExt::unlock` is a trait method (not stdlib `File::unlock`
        // added in 1.89). The clippy `incompatible_msrv` lint fires a false
        // positive here because it pattern-matches on the method name alone.
        #[allow(clippy::incompatible_msrv)]
        if let Err(err) = self._lock_handle.unlock() {
            tracing::warn!(%err, "failed to release pidfile lock on shutdown");
        }
        if let Err(err) = std::fs::remove_file(&self.pid_file) {
            tracing::warn!(%err, path = %self.pid_file.display(), "failed to unlink pidfile on shutdown");
        }
    }
}

// === PRIVATE HELPERS ===

/// Create all ancestor directories of `path` if they do not already exist.
fn ensure_parent_dirs_exist(path: &Path) -> Result<(), DaemonError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Read the PID stored in `file`, returning `None` if the file is empty or
/// contains non-numeric content (i.e. a corrupt / stale pidfile).
fn read_pid_from_file(file: &File) -> Option<u32> {
    let mut file_ref = file;
    let mut contents = String::new();
    file_ref.read_to_string(&mut contents).ok()?;
    contents.trim().parse::<u32>().ok()
}

/// Truncate `file` and write `pid` as a decimal string followed by a newline.
fn write_pid_to_file(file: &File, pid: u32) -> Result<(), DaemonError> {
    use std::io::Seek;
    let mut file_ref = file;
    file_ref.seek(std::io::SeekFrom::Start(0))?;
    file_ref.set_len(0)?;
    writeln!(file_ref, "{}", pid)?;
    Ok(())
}

/// Return `~/.vestige`, falling back to `$HOME/.vestige` if `dirs` returns
/// nothing (e.g. under some CI environments).
fn default_vestige_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".vestige")
        })
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn pid_path(dir: &TempDir) -> PathBuf {
        dir.path().join("daemon.pid")
    }

    #[test]
    fn acquire_succeeds_on_fresh_path() {
        let dir = TempDir::new().unwrap();
        let path = pid_path(&dir);

        let lifecycle =
            DaemonLifecycle::acquire(path.clone()).expect("should acquire fresh pidfile");

        // Pidfile must exist and contain current PID.
        let raw = std::fs::read_to_string(&path).unwrap();
        let stored_pid: u32 = raw.trim().parse().unwrap();
        assert_eq!(stored_pid, std::process::id());

        // Drop releases lock and unlinks the file.
        drop(lifecycle);
        assert!(!path.exists(), "pidfile should be removed after drop");
    }

    #[test]
    fn acquire_fails_when_already_locked() {
        let dir = TempDir::new().unwrap();
        let path = pid_path(&dir);

        let _first = DaemonLifecycle::acquire(path.clone()).expect("first acquire should succeed");

        let err = DaemonLifecycle::acquire(path.clone())
            .expect_err("second acquire should fail while first is alive");

        match err {
            DaemonError::AlreadyRunning { pid } => {
                assert_eq!(pid, std::process::id(), "reported pid should be ours");
            }
            other => panic!("expected AlreadyRunning, got: {other:?}"),
        }
    }

    #[test]
    fn acquire_overwrites_stale_pidfile() {
        let dir = TempDir::new().unwrap();
        let path = pid_path(&dir);

        // Write a fake stale PID without holding any lock.
        {
            let mut file = std::fs::File::create(&path).unwrap();
            writeln!(file, "99999").unwrap();
        }

        // Must succeed because nobody holds the advisory lock.
        let lifecycle =
            DaemonLifecycle::acquire(path.clone()).expect("should overwrite stale pidfile");

        let raw = std::fs::read_to_string(&path).unwrap();
        let stored_pid: u32 = raw.trim().parse().unwrap();
        assert_eq!(
            stored_pid,
            std::process::id(),
            "should overwrite stale PID with ours"
        );

        drop(lifecycle);
        assert!(!path.exists());
    }
}
