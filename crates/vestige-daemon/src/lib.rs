//! Daemon runtime for Vestige V0.5 (PRD §20).
//!
//! Provides a background process that keeps the project store open and handles
//! embed ingest, scheduled jobs, and Unix socket IPC — so CLI invocations can
//! skip cold-start I/O when the daemon is running.
//!
//! # Architecture
//!
//! ```text
//! vestige-cli  ──→  vestige-daemon  ──→  vestige-engine  ──→  vestige-core
//!                        │                                         ↑
//!                        └──────────────────────────────→  vestige-store
//! ```
//!
//! The daemon owns the long-lived `Store` connection and exposes it over a
//! Unix domain socket. Each IPC message maps 1:1 to a high-level engine
//! function; the daemon never leaks raw SQL.
//!
//! # Entry point
//!
//! [`run`] is the sole public entry point. `vestige-cli` calls it from its
//! `daemon` subcommand after resolving [`DaemonOpts`] from CLI flags.
//!
//! [`run_with_cancel`] is a testable variant that accepts a `watch::Sender<bool>`
//! so tests can stop the daemon programmatically without sending a UNIX signal.
//! Sending `true` on the channel cancels all daemon tasks.
//!
//! # Cancellation design
//!
//! We use `tokio::sync::watch` rather than `Notify` for cancellation because
//! `watch` is a persistent, fan-out signal: once `true` is sent, every task
//! that was waiting OR will start waiting sees it immediately. `Notify` would
//! require either `notify_waiters` (misses tasks not yet waiting) or repeated
//! `notify_one` calls (only notifies one waiter at a time).
//!
//! # V0.5 scope
//!
//! Wave 1–3 are complete: lifecycle (pidfile + signals), IPC status file,
//! per-project workers, registry, embed scheduler. Wave 4 adds the Unix socket
//! IPC listener.

pub mod errors;
pub mod ipc;
pub mod jobs;
pub mod lifecycle;
pub mod opts;
pub mod plist;
pub mod registry;
pub mod scheduler;
pub mod workers;

pub use errors::{DaemonError, StructuredError};
pub use lifecycle::{DaemonLifecycle, ShutdownReason};
pub use opts::DaemonOpts;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{watch, Mutex};
use vestige_config::ResolvedDaemonConfig;

use crate::ipc::methods::StatusProvider;
use crate::ipc::status_file::DaemonStatus;
use crate::registry::ProjectRegistry;

/// Busy-timeout applied to every worker's SQLite connection.
///
/// 5 000 ms is long enough to survive brief WAL contention from concurrent
/// CLI/MCP processes without blocking the daemon indefinitely.
const WORKER_BUSY_TIMEOUT_MS: u32 = 5_000;

// === STATUS PROVIDER ===

/// [`StatusProvider`] implementation used in production.
///
/// Wraps the shared registry plus the immutable start metadata needed by
/// [`scheduler::build_status`] to assemble a [`DaemonStatus`] snapshot.
///
/// `tick_state_arc` is shared with the scheduler so that `daemon.status` IPC
/// responses include a populated `next_jobs[]` array (T8.3).
struct SchedulerStatusProvider {
    registry: Arc<Mutex<ProjectRegistry>>,
    started: Instant,
    started_at: String,
    config: ResolvedDaemonConfig,
    tick_state_arc: Arc<Mutex<scheduler::TickState>>,
    /// Read side of the pause watch channel so `daemon.status` reflects `paused_until`.
    pause_rx: watch::Receiver<Option<time::OffsetDateTime>>,
}

impl StatusProvider for SchedulerStatusProvider {
    fn current_status(&self) -> Pin<Box<dyn Future<Output = DaemonStatus> + Send + '_>> {
        Box::pin(async move {
            let paused_until = scheduler::format_pause(scheduler::active_pause(&self.pause_rx));
            let reg = self.registry.lock().await;
            let state = self.tick_state_arc.lock().await;
            scheduler::build_status(
                &reg,
                self.started,
                &self.started_at,
                &self.config,
                Some(&state),
                paused_until,
            )
            .await
        })
    }
}

// === LOGGING ===

/// Resolve the directory where rolling log files are written.
///
/// - If `log_file_override` is `Some(path)` and `path` is a directory, use it
///   directly; if it is a file path, use its parent directory.
/// - Otherwise, default to `~/.vestige/logs/`.
///
/// # Test helper
///
/// Pass `Some(tmp_dir.path())` in tests to keep log files in a `TempDir` so
/// they never touch the real `~/.vestige/` tree.
pub fn resolve_log_dir(log_file_override: Option<&Path>) -> PathBuf {
    if let Some(p) = log_file_override {
        if p.is_dir() {
            return p.to_path_buf();
        }
        // If the override is a file path (legacy `--log-file` flag), use its
        // parent directory so the rolling appender writes siblings there.
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                return parent.to_path_buf();
            }
        }
        // Caller passed a bare filename with no directory component — fall through.
    }

    // Default: ~/.vestige/logs/
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".vestige").join("logs"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".vestige")
                .join("logs")
        })
}

/// Initialise the daemon's rolling-file tracing subscriber.
///
/// Writes structured log lines to `<log_dir>/daemon.log.<YYYY-MM-DD>` via
/// [`tracing_appender::rolling::daily`]. The level is controlled by
/// `VESTIGE_LOG` (falls back to `info`).
///
/// # Important — keep the guard alive
///
/// Returns a [`tracing_appender::non_blocking::WorkerGuard`] that flushes any
/// buffered log lines when dropped. Callers MUST bind it to a local variable
/// that lives for the whole daemon lifetime:
///
/// ```ignore
/// let _log_guard = init_rolling_logger(&log_dir);
/// // ... daemon runs ...
/// // _log_guard drops here, flushing final writes.
/// ```
///
/// Silently no-ops if a global subscriber has already been set (e.g. in tests
/// that call `run_with_cancel` directly after installing their own subscriber).
fn init_rolling_logger(log_dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let file_appender = tracing_appender::rolling::daily(log_dir, "daemon.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter =
        EnvFilter::try_from_env("VESTIGE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false));

    // Ignore the error — it means a subscriber is already installed (tests,
    // or the CLI initialised one before forking). The daemon still runs; it
    // just logs to wherever the existing subscriber writes.
    let _ = tracing::subscriber::set_global_default(subscriber);

    guard
}

// === PUBLIC API ===

/// Start the daemon runtime and park until a UNIX signal arrives.
///
/// Acquires the pidfile lock and hooks `SIGTERM` / `SIGINT` to the cancellation
/// channel. Delegates to [`run_with_cancel`] for the actual runtime.
pub async fn run(opts: DaemonOpts) -> Result<(), DaemonError> {
    // Set up the rolling-file subscriber before anything else so all startup
    // logs land in the file. `_log_guard` must live until `run` returns so the
    // non-blocking writer flushes on graceful shutdown.
    let log_dir = resolve_log_dir(opts.log_file.as_deref());
    std::fs::create_dir_all(&log_dir).ok();
    let _log_guard = init_rolling_logger(&log_dir);

    tracing::info!(foreground = opts.foreground, "vestige-daemon starting");

    let pid_path = lifecycle::DaemonLifecycle::resolve_pid_path(opts.pid_file.as_deref());
    let lifecycle = lifecycle::DaemonLifecycle::acquire(pid_path)?;

    let (cancel_tx, cancel_rx) = watch::channel(false);

    // Spawn shutdown-signal watcher: sets `true` on the cancel channel when a
    // signal arrives. All tasks watching `cancel_rx` will see this immediately.
    tokio::spawn(async move {
        let reason = lifecycle.wait_for_shutdown().await;
        tracing::info!(?reason, "vestige-daemon shutting down");
        cancel_tx.send(true).ok();
    });

    run_with_cancel(opts, cancel_rx).await
}

/// Start the daemon runtime with an explicit cancellation receiver.
///
/// Called by tests that need to stop the daemon programmatically. Send `true`
/// on the paired `watch::Sender<bool>` to cancel all tasks. The watch channel
/// persists the cancellation state so tasks that start late still see it.
///
/// # Steps
///
/// 1. Resolves daemon config (embed cadence etc.).
/// 2. Builds a [`ProjectRegistry`], discovers existing project DBs. Each
///    project's embedding provider is resolved from its own `.vestige/config.toml`
///    at worker spawn time.
/// 3. Spawns the IPC server and the scheduler as parallel tokio tasks.
/// 4. Awaits both tasks — they exit when `cancel_rx` holds `true`.
pub async fn run_with_cancel(
    opts: DaemonOpts,
    cancel_rx: watch::Receiver<bool>,
) -> Result<(), DaemonError> {
    // Resolve daemon config — prefer an explicit override (test escape hatch)
    // over reading from disk. Production callers always pass `config_override: None`.
    let config = match opts.config_override.clone() {
        Some(cfg) => cfg,
        None => vestige_config::daemon_config_for(None),
    };

    // Build registry and discover all project DBs.
    // Each project's embedding provider is resolved from its own
    // `.vestige/config.toml` at worker spawn time (T8.1).
    // `opts.projects_root` lets tests supply an empty TempDir to avoid
    // inheriting real project workers (which may have WAL locks or latency).
    let mut registry = ProjectRegistry::new(WORKER_BUSY_TIMEOUT_MS);
    match opts.projects_root {
        Some(ref root) => {
            registry.discover_and_spawn_in(root)?;
        }
        None => {
            registry.discover_and_spawn()?;
        }
    }
    let registry_mutex = Arc::new(Mutex::new(registry));

    // RFC-3339 start timestamp embedded in every status snapshot.
    let started = Instant::now();
    let started_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let status_path = ipc::status_file::resolve_status_path(opts.status_file.as_deref());
    let socket_path = ipc::server::resolve_socket_path(opts.socket_path.as_deref());

    // T8.4 — watch channel for live config reload.
    // `config_tx` is forwarded to the IPC dispatcher so `daemon.reload_config`
    // can push new cadences to the scheduler without a restart.
    let (config_tx, config_rx) = watch::channel(config.clone());

    // Pause channel (V0.5.2): `daemon.pause` / `daemon.resume` push an absolute resume
    // instant (or `None`) here; the scheduler reads it at each job-tick boundary and the
    // status provider surfaces it as `paused_until`.
    let (pause_tx, pause_rx) = watch::channel::<Option<time::OffsetDateTime>>(None);

    // T8.3 — shared TickState allows the IPC `daemon.status` response to include
    // a populated `next_jobs[]` array, not just the scheduler's own 5-second writes.
    let initial_tick_state = scheduler::TickState::new(
        time::OffsetDateTime::now_utc(),
        started,
        std::time::Duration::from_secs(config.embed_sweep_interval_secs),
        std::time::Duration::from_secs(config.trace_prune_interval_secs),
        std::time::Duration::from_secs(config.candidate_ttl_sweep_interval_secs),
        (config.session_log_scan_interval_secs > 0)
            .then(|| std::time::Duration::from_secs(config.session_log_scan_interval_secs)),
    );
    let shared_tick_state: Arc<Mutex<scheduler::TickState>> =
        Arc::new(Mutex::new(initial_tick_state));

    // Build the StatusProvider that the IPC server uses to answer daemon.status.
    let status_provider: Arc<dyn StatusProvider> = Arc::new(SchedulerStatusProvider {
        registry: registry_mutex.clone(),
        started,
        started_at: started_at.clone(),
        config: config.clone(),
        tick_state_arc: Arc::clone(&shared_tick_state),
        pause_rx: pause_rx.clone(),
    });

    // Spawn the IPC server as a background tokio task.
    let ipc_handle = tokio::spawn({
        let registry = registry_mutex.clone();
        let status_provider = status_provider.clone();
        let cancel = cancel_rx.clone();
        let ttl_days = config.candidate_ttl_days;
        async move {
            if let Err(e) = ipc::server::run(
                socket_path,
                registry,
                status_provider,
                ttl_days,
                config_tx,
                pause_tx,
                cancel,
            )
            .await
            {
                tracing::error!(error = ?e, "ipc server exited with error");
            }
        }
    });

    // Spawn the scheduler as a background tokio task.
    let scheduler_handle = tokio::spawn({
        let registry = registry_mutex.clone();
        let cancel = cancel_rx.clone();
        let status_path = status_path.clone();
        let started_at = started_at.clone();
        let tick_state = Arc::clone(&shared_tick_state);
        async move {
            scheduler::run(
                registry,
                config_rx,
                pause_rx,
                tick_state,
                status_path,
                started_at,
                cancel,
            )
            .await;
        }
    });

    tracing::info!(pid = std::process::id(), "vestige-daemon started");

    // Both tasks exit when `cancel_rx` holds `true`. Await them for clean shutdown.
    let _ = tokio::join!(ipc_handle, scheduler_handle);

    // Drop the StatusProvider (holds a clone of registry_mutex) so that
    // try_unwrap below sees only one strong reference.
    drop(status_provider);

    // Gracefully shut down all worker threads before dropping the registry.
    // ProjectWorker::Drop calls blocking thread::join while the sender is still
    // alive — without explicit shutdown this deadlocks on the tokio thread.
    // After the tasks above have exited and status_provider is dropped, the
    // registry_mutex Arc should have exactly one strong reference (this scope).
    if let Ok(mutex) = Arc::try_unwrap(registry_mutex) {
        let registry = mutex.into_inner();
        registry.shutdown_all().await;
    } else {
        tracing::warn!(
            "registry Arc still has multiple owners at shutdown; workers may not join cleanly"
        );
    }

    tracing::info!("vestige-daemon stopped");
    Ok(())
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tempdir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn resolve_log_dir_none_override_returns_vestige_logs_suffix() {
        // Without an override the path ends with .vestige/logs. We can't
        // assert the prefix because HOME varies per machine, so just check
        // the suffix and that it's absolute.
        let dir = resolve_log_dir(None);
        assert!(dir.is_absolute(), "expected absolute path, got {dir:?}");
        let s = dir.to_string_lossy();
        assert!(
            s.ends_with(".vestige/logs") || s.ends_with(".vestige\\logs"),
            "expected path to end with .vestige/logs, got {s}"
        );
    }

    #[test]
    fn resolve_log_dir_directory_override_returns_it_unchanged() {
        let tmp = make_tempdir();
        let dir = resolve_log_dir(Some(tmp.path()));
        assert_eq!(dir, tmp.path());
    }

    #[test]
    fn resolve_log_dir_file_override_returns_parent() {
        let tmp = make_tempdir();
        let file = tmp.path().join("daemon.log");
        // File doesn't need to exist for resolve_log_dir — it checks is_dir(),
        // not is_file(), so a non-existent file path falls to the parent branch.
        let dir = resolve_log_dir(Some(&file));
        assert_eq!(dir, tmp.path());
    }

    #[test]
    fn resolve_log_dir_bare_filename_falls_back_to_default() {
        // A bare filename has no parent component; should fall through to the
        // default `~/.vestige/logs` path.
        let bare = std::path::Path::new("daemon.log");
        let dir = resolve_log_dir(Some(bare));
        let s = dir.to_string_lossy();
        assert!(
            s.ends_with(".vestige/logs") || s.ends_with(".vestige\\logs"),
            "expected fallback to .vestige/logs, got {s}"
        );
    }
}
