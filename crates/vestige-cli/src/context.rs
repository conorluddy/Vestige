//! Shared CLI helpers for resolving the active Vestige project from cwd.
//!
//! [`load`] is the primary entry point: it walks up from `cwd` to find
//! `.vestige/config.toml`, derives the [`ProjectId`], opens the
//! `~/.vestige/projects/<id>/memory.sqlite` store, and returns a
//! [`ProjectContext`] ready for use by any command handler.

use anyhow::{Context, Result};
use vestige_config::{discover_config, EmbeddingsConfigSection, VestigeConfig};
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
        embeddings_config_from_section(self.config.embeddings.as_ref())
    }
}

/// Build an embedding provider from explicit parameters.
///
/// CLI flags override the config section (e.g. `vestige embed --provider ollama`).
/// When all three params are `None` and no config section is present, defaults
/// to the `"fake"` provider.
pub fn embedding_provider(
    provider: Option<&str>,
    model: Option<&str>,
    dimensions: Option<usize>,
) -> Result<Box<dyn vestige_embed::EmbeddingProvider>> {
    let cfg = EmbeddingsConfig {
        provider: provider.unwrap_or("fake").to_string(),
        model: model.map(|s| s.to_owned()),
        dimensions,
    };
    vestige_embed::build_provider(&cfg).map_err(|e| anyhow::anyhow!("embedding provider: {e}"))
}

/// Map a typed `[embeddings]` config section onto `vestige-embed`'s
/// runtime config. Single source of truth; used by both CLI and MCP paths.
pub fn embeddings_config_from_section(
    section: Option<&EmbeddingsConfigSection>,
) -> EmbeddingsConfig {
    match section {
        Some(s) => EmbeddingsConfig {
            provider: s.provider.clone().unwrap_or_else(|| "fake".to_string()),
            model: s.model.clone(),
            dimensions: s.dimensions,
        },
        None => EmbeddingsConfig {
            provider: "fake".to_string(),
            model: None,
            dimensions: None,
        },
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
