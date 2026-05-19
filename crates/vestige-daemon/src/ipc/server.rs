//! Daemon control-plane Unix-domain socket server.
//!
//! Newline-delimited JSON-RPC 2.0. One request → one response. Connections are
//! short-lived; clients connect, send a request, read the response, disconnect.
//!
//! # Socket cleanup
//!
//! A stale socket file from a previously crashed daemon is removed before
//! binding. The single-instance pidfile lock (Wave 1 / `lifecycle.rs`) prevents
//! two live daemons from racing here.
//!
//! # Cancellation
//!
//! [`run`] loops until `cancel` is notified, then performs a best-effort
//! cleanup of the socket file before returning.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, watch};

use crate::errors::DaemonError;
use crate::ipc::methods;
use crate::registry::ProjectRegistry;

// === PUBLIC API ===

/// Resolve `~/.vestige/daemon.sock`, honouring an optional override.
///
/// Override is used in tests to keep socket files isolated in a
/// `tempfile::TempDir`.
pub fn resolve_socket_path(override_path: Option<&Path>) -> PathBuf {
    if let Some(path) = override_path {
        return path.to_path_buf();
    }
    default_vestige_dir().join("daemon.sock")
}

/// Start the Unix-domain socket listener and serve requests until cancelled.
///
/// Steps:
/// 1. Remove any stale socket file at `socket_path`.
/// 2. Ensure the parent directory exists.
/// 3. Bind a [`UnixListener`].
/// 4. Accept connections in a `tokio::select!` loop until `cancel` holds `true`.
/// 5. Best-effort cleanup: remove the socket file on graceful shutdown.
///
/// Each accepted connection is handled in its own spawned task so slow clients
/// do not block the accept loop.
pub async fn run(
    socket_path: PathBuf,
    registry: Arc<Mutex<ProjectRegistry>>,
    status_provider: Arc<dyn methods::StatusProvider>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), DaemonError> {
    // Remove any stale socket from a previously crashed daemon.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).map_err(|e| {
            DaemonError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to remove stale socket at {}: {e}",
                    socket_path.display()
                ),
            ))
        })?;
    }

    // Ensure parent directory exists.
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!(socket = %socket_path.display(), "ipc server listening");

    loop {
        tokio::select! {
            biased;
            result = cancel.changed() => {
                if result.is_err() || *cancel.borrow() {
                    tracing::info!("ipc server: cancellation received — stopping");
                    break;
                }
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        tokio::spawn(handle_connection(
                            stream,
                            Arc::clone(&registry),
                            Arc::clone(&status_provider),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(error = ?e, "accept failed; continuing");
                    }
                }
            }
        }
    }

    // Best-effort cleanup.
    if let Err(e) = std::fs::remove_file(&socket_path) {
        tracing::warn!(
            socket = %socket_path.display(),
            error = ?e,
            "failed to remove socket file on shutdown"
        );
    }

    Ok(())
}

// === PRIVATE HELPERS ===

/// Handle one client connection: read one line, dispatch, write one response.
async fn handle_connection(
    stream: UnixStream,
    registry: Arc<Mutex<ProjectRegistry>>,
    status_provider: Arc<dyn methods::StatusProvider>,
) {
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    match reader.read_line(&mut line).await {
        Ok(0) => {
            // Client closed the connection before sending anything.
            tracing::debug!("client disconnected before sending a request");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = ?e, "error reading from client connection");
            return;
        }
    }

    let response = match serde_json::from_str::<methods::JsonRpcRequest>(line.trim_end()) {
        Ok(req) => methods::dispatch(registry, &*status_provider, req).await,
        Err(e) => methods::parse_error_response(e.to_string()),
    };

    let payload = match serde_json::to_string(&response) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = ?e, "failed to serialize response — connection dropped");
            return;
        }
    };

    let _ = write_half.write_all(payload.as_bytes()).await;
    let _ = write_half.write_all(b"\n").await;
    let _ = write_half.shutdown().await;
}

/// Return `~/.vestige`, falling back to `$HOME/.vestige` in minimal environments.
fn default_vestige_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".vestige")
        })
}
