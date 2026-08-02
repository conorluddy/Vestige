//! `vestige daemon kick {embed|prune|ttl}` — send a one-off job request over the IPC socket.
//!
//! # T11 dependency
//!
//! `vestige_daemon::ipc::server::resolve_socket_path` is not yet exposed — T11
//! (the Unix socket IPC listener) is landing in parallel. Until T11 merges, this
//! command hard-codes the default socket path `~/.vestige/daemon.sock`.
//! TODO(T11): replace `default_socket_path()` with
//! `vestige_daemon::ipc::server::resolve_socket_path(None)` once exported.

use clap::{Args, ValueEnum};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// === TYPES ===

#[derive(Args, Debug)]
pub struct KickArgs {
    /// Which background job to trigger immediately.
    #[arg(value_enum)]
    pub job: KickJob,
    /// Limit to a specific project ID; omit to kick all projects.
    #[arg(long)]
    pub project: Option<String>,
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum KickJob {
    /// Sweep unembedded memory representations through the provider.
    Embed,
    /// Evict old query-trace events to keep the trace table bounded.
    Prune,
    /// Mark overdue assimilation candidates as rejected via TTL policy.
    Ttl,
    /// Scan local session transcripts and propose candidates (V0.5.4).
    Scan,
}

// === PUBLIC API ===

pub fn run(args: KickArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(kick_async(args))
}

// === PRIVATE HELPERS ===

async fn kick_async(args: KickArgs) -> anyhow::Result<()> {
    // TODO(T11): replace with vestige_daemon::ipc::server::resolve_socket_path(None)
    let socket_path = default_socket_path();

    let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        anyhow::anyhow!(
            "could not reach daemon socket at {}: {e}",
            socket_path.display()
        )
    })?;

    let job_str = match args.job {
        KickJob::Embed => "embed",
        KickJob::Prune => "prune",
        KickJob::Ttl => "ttl",
        KickJob::Scan => "scan",
    };

    let mut params = json!({ "job": job_str });
    if let Some(project) = args.project {
        params["project_id"] = json!(project);
    }

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "daemon.kick",
        "params": params,
    });
    let request_line = serde_json::to_string(&request)? + "\n";
    stream.write_all(request_line.as_bytes()).await?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;

    let response: serde_json::Value = serde_json::from_str(&response_line)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if let Some(err) = response.get("error") {
        println!(
            "kick failed: {} ({})",
            err["message"].as_str().unwrap_or("unknown"),
            err["code"].as_i64().unwrap_or(-1)
        );
    } else if let Some(result) = response.get("result") {
        println!(
            "kick {} queued — projects_queued={}",
            job_str,
            result["projects_queued"].as_u64().unwrap_or(0)
        );
    }

    Ok(())
}

/// Default IPC socket path: `~/.vestige/daemon.sock`.
///
/// TODO(T11): remove once `vestige_daemon::ipc::server::resolve_socket_path` is exported.
fn default_socket_path() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige").join("daemon.sock"))
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".vestige")
                .join("daemon.sock")
        })
}
