//! Shared CLI helpers for resolving the active Vestige project from cwd.

use anyhow::{Context, Result};
use vestige_config::discover_config;
use vestige_core::ProjectId;
use vestige_store::Store;

pub struct ProjectContext {
    pub project_id: ProjectId,
    pub store: Store,
}

pub fn load() -> Result<ProjectContext> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (_path, config) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;
    let project_id = config.project_id()?;
    let storage_path = config.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;
    Ok(ProjectContext { project_id, store })
}
