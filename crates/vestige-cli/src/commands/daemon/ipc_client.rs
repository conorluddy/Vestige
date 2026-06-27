//! Tiny JSON-RPC-over-Unix-socket client shared by the `daemon` controller subcommands.
//!
//! Each subcommand (`kick`, `pause`, `resume`, …) is a thin adapter: build params, send one
//! request, read one response. This module owns the socket path resolution and the
//! one-request/one-response round-trip so those adapters stay focused on formatting.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Default IPC socket path: `~/.vestige/daemon.sock`.
pub fn default_socket_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige").join("daemon.sock"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".vestige")
                .join("daemon.sock")
        })
}

/// Send a single JSON-RPC 2.0 `method` call with `params` and return the parsed response.
///
/// A connection failure is mapped to an actionable error directing the user to start the
/// daemon — the daemon-not-running case is the common one.
pub async fn call(method: &str, params: Value) -> Result<Value> {
    let socket_path = default_socket_path();
    let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        anyhow::anyhow!(
            "could not reach the daemon socket at {} ({e}) — is the daemon running? Try `vestige daemon start`.",
            socket_path.display()
        )
    })?;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let request_line = serde_json::to_string(&request)? + "\n";
    stream.write_all(request_line.as_bytes()).await?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;
    Ok(serde_json::from_str(&response_line)?)
}
