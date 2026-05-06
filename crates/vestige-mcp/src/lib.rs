//! MCP adapter — six high-level tools over project memory (PRD §13.2).
//!
//! Thin adapter crate: each tool maps 1:1 to a `vestige-core` or
//! `vestige-engine` function. No SQL, no destructive defaults, no
//! cross-project access. Errors are always structured `{code, message,
//! retryable}` so coding agents can branch on them.
//!
//! # Tool surface (PRD §13.2)
//!
//! | Tool | File | Purpose |
//! |------|------|---------|
//! | `vestige_bootstrap` | `tools/bootstrap.rs` | Standing context at session start |
//! | `vestige_search` | `tools/search.rs` | Lexical / semantic / hybrid search |
//! | `vestige_expand` | `tools/expand.rs` | Full content at chosen depth |
//! | `vestige_get_project_context` | `tools/project_context.rs` | Budget-bounded context pack |
//! | `vestige_record_observation` | `tools/record_observation.rs` | Write a new observation |
//! | `vestige_record_decision` | `tools/record_decision.rs` | Write a new decision |
//!
//! # Entry point
//!
//! [`run`] resolves the project config, opens the store, and starts the
//! `rmcp` stdio server. `vestige-cli` calls this from its `mcp` subcommand.

mod server;
mod tools;

use std::path::PathBuf;

use anyhow::{Context, Result};
use rmcp::{transport::stdio, ServiceExt};
use vestige_config::discover_config;
use vestige_store::Store;

pub use server::VestigeServer;
pub use tools::get_candidate::GetCandidateParams;
pub use tools::list_candidates::ListCandidatesParams;
pub use tools::propose_candidate::{ProposeCandidateParams, ProposeSource};
pub use tools::search::SearchParams;

/// Options forwarded from `vestige mcp` CLI flags.
pub struct McpOptions {
    /// When `true`, write tools (`record_observation`, `record_decision`) are
    /// disabled and return `READ_ONLY` errors.
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
