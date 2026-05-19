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
pub mod opts;

pub use errors::{DaemonError, StructuredError};
pub use opts::DaemonOpts;

/// Start the daemon runtime.
///
/// Resolves project config, opens the store, and (once Wave 2 fills this in)
/// begins accepting IPC connections on the Unix socket. Currently a no-op
/// stub that logs and returns.
pub async fn run(opts: DaemonOpts) -> Result<(), DaemonError> {
    tracing::info!(foreground = opts.foreground, "vestige-daemon starting");
    Ok(())
}
