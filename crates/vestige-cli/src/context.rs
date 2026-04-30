//! Shared CLI helpers for resolving the active Vestige project from cwd.

use anyhow::{Context, Result};
use vestige_config::{discover_config, EmbeddingsConfigSection, VestigeConfig};
use vestige_core::ProjectId;
use vestige_embed::EmbeddingsConfig;
use vestige_store::Store;

pub struct ProjectContext {
    pub config: VestigeConfig,
    pub project_id: ProjectId,
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
