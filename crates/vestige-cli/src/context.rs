//! Shared CLI helpers for resolving the active Vestige project from cwd.
//!
//! [`load`] is the primary entry point: it walks up from `cwd` to find
//! `.vestige/config.toml`, derives the [`ProjectId`], opens the
//! `~/.vestige/projects/<id>/memory.sqlite` store, and returns a
//! [`ProjectContext`] ready for use by any command handler.

use anyhow::{Context, Result};
use vestige_config::{discover_config, embeddings_config_for, VestigeConfig};
use vestige_core::ProjectId;
use vestige_embed::EmbeddingsConfig;
use vestige_store::Store;

/// Runtime context for a resolved Vestige project.
///
/// Built by [`load`] from the nearest `.vestige/config.toml`. Commands borrow
/// `store` for all read/write operations; `project_id` scopes every query so
/// memory from other projects is never accessible.
pub struct ProjectContext {
    /// Parsed `.vestige/config.toml` for the current repo.
    pub config: VestigeConfig,
    /// Stable typed identifier for this project (`proj_<slug-or-hash>`).
    pub project_id: ProjectId,
    /// Open handle to `~/.vestige/projects/<project_id>/memory.sqlite`.
    pub store: Store,
}

impl ProjectContext {
    /// Resolve the embedding provider config from the typed
    /// `[embeddings]` section in `.vestige/config.toml`.
    ///
    /// Defaults to `provider = "fake"` when the section is absent so
    /// `vestige embed --all` works out of the box.
    pub fn resolve_embeddings_config(&self) -> EmbeddingsConfig {
        embeddings_config_for(self.config.embeddings.as_ref())
    }

    /// Build the configured provider, applying explicit CLI overrides.
    /// Changing providers resets model/dimensions to the new backend's defaults
    /// unless a model override is also supplied.
    pub fn embedding_provider(
        &self,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Box<dyn vestige_embed::EmbeddingProvider>> {
        let mut cfg = self.resolve_embeddings_config();
        if let Some(provider) = provider {
            if provider != cfg.provider {
                cfg.model = None;
                cfg.dimensions = None;
            }
            cfg.provider = provider.to_owned();
        }
        if let Some(model) = model {
            if cfg.model.as_deref() != Some(model) {
                cfg.dimensions = None;
            }
            cfg.model = Some(model.to_owned());
        }
        vestige_embed::build_provider(&cfg).map_err(anyhow::Error::from)
    }
}

/// Resolve the active project from `cwd` and open its store.
///
/// Fails with an actionable message if no `.vestige/config.toml` is found
/// (suggesting `vestige init`) or if the store cannot be opened.
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
