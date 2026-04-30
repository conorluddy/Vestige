//! Config TOML data model — structs, defaults, and the `VestigeConfig` methods.
//! Serde-serializable; no I/O.

use serde::{Deserialize, Serialize};

use vestige_core::ProjectId;

use crate::{ConfigError, Result};

fn default_scope() -> String {
    "project".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VestigeConfig {
    pub project_id: String,
    pub project_name: String,
    #[serde(default = "default_scope")]
    pub scope: String,

    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub recall: RecallConfig,
    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub embeddings: Option<EmbeddingsConfigSection>,

    #[serde(default)]
    pub search: Option<SearchConfigSection>,
}

/// Configuration for the embedding provider (`[embeddings]` in `.vestige/config.toml`).
///
/// All fields are optional — omitting the section keeps behaviour identical to V0
/// (lexical FTS only, `"fake"` provider for tests).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct EmbeddingsConfigSection {
    /// `"fake"` | `"fastembed"` | `"ollama"`. Default: `"fake"` (works out of the box for
    /// tests; switch to `"fastembed"` for real semantic recall once you've installed with
    /// `--features fastembed`).
    pub provider: Option<String>,
    /// Model identifier passed to the provider. Defaults to the provider's recommended model.
    pub model: Option<String>,
    /// Vector dimensions. Defaults to the provider's native dimensions.
    pub dimensions: Option<usize>,
    /// Which representations to embed by default. PRD §6.2: `["summary", "compressed"]`.
    pub default_representations: Option<Vec<String>>,
}

/// Configuration for search behaviour (`[search]` in `.vestige/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SearchConfigSection {
    /// `"lexical"` | `"semantic"` | `"hybrid"`. Default: `"lexical"` (backwards-compat with V0).
    pub default_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    pub mode: String,
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mode: "user_data".into(),
            path: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallConfig {
    pub default_depth: String,
    pub max_results: u32,
    pub include_global_preferences: bool,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            default_depth: "one_liner".into(),
            max_results: 8,
            include_global_preferences: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpConfig {
    pub allow_record_observation: bool,
    pub allow_record_decision: bool,
    pub allow_forget: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            allow_record_observation: true,
            allow_record_decision: true,
            allow_forget: false,
        }
    }
}

impl VestigeConfig {
    pub fn project_id(&self) -> Result<ProjectId> {
        use std::str::FromStr;
        ProjectId::from_str(&self.project_id)
            .map_err(|e| ConfigError::InvalidProjectId(e.to_string()))
    }

    /// Storage path with `~` expanded to the user's home dir.
    pub fn resolved_storage_path(&self) -> Result<std::path::PathBuf> {
        crate::paths::expand_tilde(&self.storage.path)
    }
}
