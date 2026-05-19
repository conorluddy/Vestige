//! Typed error enum for `vestige-daemon`.
//!
//! [`DaemonError`] covers all failure modes the daemon runtime can surface.
//! Every variant maps to a [`StructuredError`] — the `{code, message, retryable}`
//! envelope the IPC layer (Wave 4) returns to agents. This mirrors the contract
//! used at the MCP boundary (PRD §14.3).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// === TYPES ===

/// Machine-parseable error envelope returned over the IPC socket.
///
/// Mirrors the MCP structured error shape so agent code that already handles
/// `{code, message, retryable}` from the MCP layer needs no extra branching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

// === PUBLIC API ===

/// All failure modes for the `vestige-daemon` runtime.
///
/// Wraps lower-layer errors from `vestige-store`, `vestige-config`, and
/// `vestige-core` via `#[from]` so `?` works across crate boundaries.
/// Call [`DaemonError::structured`] before sending an error to a client
/// over the IPC socket.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// A persistence operation failed in `vestige-store`.
    #[error("vestige-store error: {0}")]
    Store(#[from] vestige_store::StoreError),

    /// A config load or resolution failed in `vestige-config`.
    #[error("vestige-config error: {0}")]
    Config(#[from] vestige_config::ConfigError),

    /// A domain-level failure bubbled up from `vestige-core`.
    #[error("vestige-core error: {0}")]
    Core(#[from] vestige_core::CoreError),

    /// A filesystem or OS I/O operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A daemon instance is already running with the given PID.
    #[error("another vestige-daemon instance is running (pid={pid})")]
    AlreadyRunning { pid: u32 },

    /// The Unix domain socket is not reachable at the given path.
    /// Typically means the daemon is not running or the socket was removed.
    #[error("daemon socket not reachable at {path}")]
    SocketUnreachable { path: PathBuf },

    /// The client requested a project that has not been registered with the daemon.
    #[error("project not registered: {project_id}")]
    ProjectNotRegistered { project_id: String },

    /// A scheduled or background job failed.
    #[error("job failed: {job} — {reason}")]
    JobFailed { job: String, reason: String },
}

impl DaemonError {
    /// Convert this error into the structured `{code, message, retryable}` envelope
    /// for the IPC layer. Agents can branch on `code` without parsing `message`.
    pub fn structured(&self) -> StructuredError {
        let (code, retryable) = match self {
            DaemonError::Store(_) => ("STORE_ERROR", false),
            DaemonError::Config(_) => ("CONFIG_ERROR", false),
            DaemonError::Core(_) => ("CORE_ERROR", false),
            DaemonError::Io(_) => ("IO_ERROR", true),
            DaemonError::AlreadyRunning { .. } => ("ALREADY_RUNNING", false),
            DaemonError::SocketUnreachable { .. } => ("SOCKET_UNREACHABLE", true),
            DaemonError::ProjectNotRegistered { .. } => ("PROJECT_NOT_REGISTERED", false),
            DaemonError::JobFailed { .. } => ("JOB_FAILED", false),
        };

        StructuredError {
            code: code.to_string(),
            message: self.to_string(),
            retryable,
        }
    }
}
