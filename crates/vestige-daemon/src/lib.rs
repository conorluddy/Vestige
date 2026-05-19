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

use std::sync::Arc;
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};

/// Busy-timeout applied to every worker's SQLite connection.
///
/// 5 000 ms is long enough to survive brief WAL contention from concurrent
/// CLI/MCP processes without blocking the daemon indefinitely.
const WORKER_BUSY_TIMEOUT_MS: u32 = 5_000;

/// Start the daemon runtime.
///
/// 1. Acquires the pidfile lock — fails fast with [`DaemonError::AlreadyRunning`]
///    if another instance is alive.
/// 2. Builds a [`registry::ProjectRegistry`], sets an embedding provider
///    (`FakeEmbeddingProvider` for V0.5; real provider selection arrives in V0.6),
///    and discovers all existing project DBs.
/// 3. Spawns the [`scheduler`] as a background tokio task.
/// 4. Parks on the shutdown signal.
/// 5. Notifies the scheduler to stop, awaits it, then returns.
pub async fn run(opts: DaemonOpts) -> Result<(), DaemonError> {
    tracing::info!(foreground = opts.foreground, "vestige-daemon starting");

    let pid_path = lifecycle::DaemonLifecycle::resolve_pid_path(opts.pid_file.as_deref());
    let lifecycle = lifecycle::DaemonLifecycle::acquire(pid_path)?;

    // Resolve daemon config — no host-level config file yet so we use the
    // documented defaults (embed_sweep_interval_secs = 600).
    let config = vestige_config::daemon_config_for(None);

    // Embedding provider — V0.5 defaults to `fake` (deterministic, no model
    // download). Real provider selection (fastembed/ollama) is wired in V0.6
    // when the daemon reads the project's `[embeddings]` config section.
    let provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>> =
        Some(Arc::new(FakeEmbeddingProvider::default()));

    // Build registry and discover all project DBs under ~/.vestige/projects/.
    let mut registry = registry::ProjectRegistry::new(WORKER_BUSY_TIMEOUT_MS);
    registry.set_provider(provider);
    registry.discover_and_spawn()?;
    let registry = Arc::new(registry);

    // RFC-3339 start timestamp embedded in every status snapshot.
    let started_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let status_path = ipc::status_file::resolve_status_path(opts.status_file.as_deref());

    // Cancellation notifier: the scheduler loop exits when this fires.
    let cancel = Arc::new(tokio::sync::Notify::new());

    // Spawn the scheduler as a background tokio task.
    let scheduler_handle = tokio::spawn({
        let registry = registry.clone();
        let cancel = cancel.clone();
        let status_path = status_path.clone();
        let started_at = started_at.clone();
        async move {
            scheduler::run(registry, config, status_path, started_at, cancel).await;
        }
    });

    tracing::info!(pid = std::process::id(), "vestige-daemon started");

    let reason = lifecycle.wait_for_shutdown().await;
    tracing::info!(?reason, "vestige-daemon shutting down");

    cancel.notify_waiters();
    let _ = scheduler_handle.await;

    Ok(())
}
