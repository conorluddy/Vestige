//! Shared CLI helpers for resolving the active Vestige project from cwd.

use anyhow::{Context, Result};
use vestige_config::{discover_config, VestigeConfig};
use vestige_core::ProjectId;
use vestige_store::Store;

pub struct ProjectContext {
    pub config: VestigeConfig,
    pub project_id: ProjectId,
    pub store: Store,
}

/// Build an embedding provider from explicit parameters.
///
/// `VestigeConfig` does not yet have a first-class `[embeddings]` section —
/// PR8 will add it. Until then, callers pass provider/model/dimensions from
/// CLI flags (or `None` to get the `"fake"` default). This ensures
/// `vestige embed --all` with no config works out of the box.
pub fn embedding_provider(
    provider: Option<&str>,
    model: Option<&str>,
    dimensions: Option<usize>,
) -> Result<Box<dyn vestige_embed::EmbeddingProvider>> {
    let cfg = vestige_embed::EmbeddingsConfig {
        provider: provider.unwrap_or("fake").to_string(),
        model: model.map(|s| s.to_owned()),
        dimensions,
    };
    vestige_embed::build_provider(&cfg).map_err(|e| anyhow::anyhow!("embedding provider: {e}"))
}

pub fn load() -> Result<ProjectContext> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (_path, config) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;
    let project_id = config.project_id()?;
    let storage_path = config.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;
    Ok(ProjectContext {
        config,
        project_id,
        store,
    })
}
