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
//! This skeleton is T1 of the V0.5 wave. Business logic (socket listener,
//! embed scheduler, PID file management, signal handling) is added in
//! subsequent Wave 2–4 tasks.

pub mod errors;
pub mod ipc;
pub mod lifecycle;
pub mod opts;
pub mod registry;
pub mod workers;

pub use errors::{DaemonError, StructuredError};
pub use lifecycle::{DaemonLifecycle, ShutdownReason};
pub use opts::DaemonOpts;

/// Start the daemon runtime.
///
/// Acquires the pidfile lock (fails fast with [`DaemonError::AlreadyRunning`]
/// if another instance is alive), parks on the shutdown signal, then returns.
///
/// Wave 3 wires the IPC socket and embed scheduler between acquire and
/// `wait_for_shutdown`.
pub async fn run(opts: DaemonOpts) -> Result<(), DaemonError> {
    tracing::info!(foreground = opts.foreground, "vestige-daemon starting");

    let pid_path = DaemonLifecycle::resolve_pid_path(opts.pid_file.as_deref());
    let lifecycle = DaemonLifecycle::acquire(pid_path)?;

    tracing::info!(pid = std::process::id(), "vestige-daemon started");

    // Wave 3 wires scheduler and IPC listener here.
    let reason = lifecycle.wait_for_shutdown().await;

    tracing::info!(?reason, "vestige-daemon shutting down");
    Ok(())
}
