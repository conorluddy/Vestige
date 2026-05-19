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
pub mod registry;
pub mod scheduler;
pub mod workers;

pub use errors::{DaemonError, StructuredError};
pub use lifecycle::{DaemonLifecycle, ShutdownReason};
pub use opts::DaemonOpts;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, watch};
use vestige_config::ResolvedDaemonConfig;
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};

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
struct SchedulerStatusProvider {
    registry: Arc<Mutex<ProjectRegistry>>,
    started: Instant,
    started_at: String,
    config: ResolvedDaemonConfig,
}

impl StatusProvider for SchedulerStatusProvider {
    fn current_status(&self) -> Pin<Box<dyn Future<Output = DaemonStatus> + Send + '_>> {
        Box::pin(async move {
            let reg = self.registry.lock().await;
            scheduler::build_status(&reg, self.started, &self.started_at, &self.config).await
        })
    }
}

// === PUBLIC API ===

/// Start the daemon runtime and park until a UNIX signal arrives.
///
/// Acquires the pidfile lock and hooks `SIGTERM` / `SIGINT` to the cancellation
/// channel. Delegates to [`run_with_cancel`] for the actual runtime.
pub async fn run(opts: DaemonOpts) -> Result<(), DaemonError> {
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
/// 2. Sets the `FakeEmbeddingProvider` (V0.5 default; real provider selection in V0.6).
/// 3. Builds a [`ProjectRegistry`], discovers existing project DBs.
/// 4. Spawns the IPC server and the scheduler as parallel tokio tasks.
/// 5. Awaits both tasks — they exit when `cancel_rx` holds `true`.
pub async fn run_with_cancel(
    opts: DaemonOpts,
    cancel_rx: watch::Receiver<bool>,
) -> Result<(), DaemonError> {
    // Resolve daemon config — no host-level config file yet so we use the
    // documented defaults (embed_sweep_interval_secs = 600).
    let config = vestige_config::daemon_config_for(None);

    // Embedding provider — V0.5 defaults to `fake` (deterministic, no model
    // download). Real provider selection (fastembed/ollama) is wired in V0.6
    // when the daemon reads the project's `[embeddings]` config section.
    let provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>> =
        Some(Arc::new(FakeEmbeddingProvider::default()));

    // Build registry and discover all project DBs.
    // `opts.projects_root` lets tests supply an empty TempDir to avoid
    // inheriting real project workers (which may have WAL locks or latency).
    let mut registry = ProjectRegistry::new(WORKER_BUSY_TIMEOUT_MS);
    registry.set_provider(provider);
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

    // Build the StatusProvider that the IPC server uses to answer daemon.status.
    let status_provider: Arc<dyn StatusProvider> = Arc::new(SchedulerStatusProvider {
        registry: registry_mutex.clone(),
        started,
        started_at: started_at.clone(),
        config: config.clone(),
    });

    // Spawn the IPC server as a background tokio task.
    let ipc_handle = tokio::spawn({
        let registry = registry_mutex.clone();
        let status_provider = status_provider.clone();
        let cancel = cancel_rx.clone();
        async move {
            if let Err(e) = ipc::server::run(socket_path, registry, status_provider, cancel).await {
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
        async move {
            scheduler::run(registry, config, status_path, started_at, cancel).await;
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
        tracing::warn!("registry Arc still has multiple owners at shutdown; workers may not join cleanly");
    }

    tracing::info!("vestige-daemon stopped");
    Ok(())
}
