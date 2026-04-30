//! MCP adapter — thin wrappers over `vestige-core` operations exposed as
//! six high-level tools (PRD §13.2). Each tool maps 1:1 to a core function;
//! no SQL, no destructive defaults, no cross-project access.

mod server;
mod tools;

use std::path::PathBuf;

use anyhow::{Context, Result};
use rmcp::{transport::stdio, ServiceExt};
use vestige_config::discover_config;
use vestige_store::Store;

pub use server::VestigeServer;

pub struct McpOptions {
    pub read_only: bool,
}

pub async fn run(opts: McpOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (config_path, config) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;
    let project_id = config.project_id()?;
    let storage_path: PathBuf = config.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;

    tracing::info!(
        project = %project_id,
        config = %config_path.display(),
        storage = %storage_path.display(),
        read_only = opts.read_only,
        "starting MCP server"
    );

    let server = VestigeServer::new(store, config, project_id, opts.read_only);
    let service = server.serve(stdio()).await.context("MCP serve")?;
    service.waiting().await.context("MCP wait")?;
    Ok(())
}
