//! `vestige status` — show the active project state.
//!
//! Prints the project name, ID, scope, config and memory-DB paths, and a
//! brief count of active vs. deleted memories. No `--json` flag (text only).

use anyhow::{Context, Result};

use vestige_config::discover_config;
use vestige_store::Store;

/// Print a project status overview to stdout.
pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (config_path, cfg) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;

    let project_id = cfg.project_id()?;
    let storage_path = cfg.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;
    let counts = store.memory_counts(&project_id)?;

    let repo_root = config_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    println!("Project:    {} ({})", cfg.project_name, cfg.project_id);
    println!("Scope:      {}", cfg.scope);
    println!("Repo root:  {}", repo_root.display());
    println!("Config:     {}", config_path.display());
    println!("Memory DB:  {}", storage_path.display());
    println!(
        "Memories:   {} active, {} deleted",
        counts.active, counts.deleted
    );
    println!("MCP:        run `vestige mcp` to expose this project to an agent over stdio");
    Ok(())
}
