//! Config TOML data model — structs, defaults, and `VestigeConfig` helpers.
//!
//! This module is pure: it owns the wire-level layout of `.vestige/config.toml`
//! and nothing else. No filesystem I/O, no process spawning.
//!
//! # Wire format
//!
//! ```toml
//! project_id   = "proj_vestige"
//! project_name = "Vestige"
//! scope        = "project"        # optional, default "project"
//!
//! [storage]
//! mode = "user_data"
//! path = "~/.vestige/projects/proj_vestige/memory.sqlite"
//!
//! [recall]
//! default_depth              = "one_liner"
//! max_results                = 8
//! include_global_preferences = false
//!
//! [mcp]
//! allow_record_observation = true
//! allow_record_decision    = true
//! allow_forget             = false
//!
//! # Optional V0.1+ sections — omit for full V0 compat:
//! # [embeddings]
//! # [search]
//! ```
//!
//! Missing optional sections deserialise to `None`; re-serialising a config
//! loaded from a V0 file will not emit those sections.

use serde::{Deserialize, Serialize};

use vestige_core::ProjectId;
use vestige_embed::EmbeddingsConfig;

use crate::{ConfigError, Result};

/// Default embedding provider when no `[embeddings]` section is set.
///
/// `"fake"` works without network or model downloads — sensible for tests
/// and first-run experience. Real semantic recall requires switching to
/// `"fastembed"` or `"ollama"` and re-running `vestige embed --all`.
const DEFAULT_PROVIDER: &str = "fake";

/// Default value for the `scope` field — `"project"`.
fn default_scope() -> String {
    "project".into()
}

// === TOP-LEVEL CONFIG ===

/// Root config struct, mirroring `.vestige/config.toml` one-to-one.
///
/// Serialised with `toml::to_string_pretty` by [`write_config`](crate::write_config);
/// deserialised with `toml::from_str` by [`read_config`](crate::read_config).
///
/// Optional sections ([`embeddings`](Self::embeddings), [`search`](Self::search))
/// use `Option<_>` rather than a defaulted struct so that a V0 config round-trips
/// without gaining new sections it didn't have.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VestigeConfig {
    /// Serialised `ProjectId`, e.g. `"proj_vestige"`.
    ///
    /// Use [`VestigeConfig::project_id`] to parse this into a typed
    /// [`ProjectId`]. The raw string is kept here so the TOML round-trip is
    /// lossless even if the prefix rules change.
    pub project_id: String,

    /// Human-readable project name written to config at `vestige init` time.
    ///
    /// Displayed in `vestige status` and used as the source display name on
    /// memories. Defaults to the directory basename when `--name` is omitted.
    pub project_name: String,

    /// Memory scope for this project. Always `"project"` in V0.
    ///
    /// Reserved for future cross-project scoping (V0.7). Do not branch on this
    /// value until that milestone lands.
    #[serde(default = "default_scope")]
    pub scope: String,

    /// SQLite storage location (TOML `[storage]`).
    #[serde(default)]
    pub storage: StorageConfig,

    /// Recall/search defaults (TOML `[recall]`).
    #[serde(default)]
    pub recall: RecallConfig,

    /// MCP capability gates (TOML `[mcp]`).
    #[serde(default)]
    pub mcp: McpConfig,

    /// Optional embedding provider config (TOML `[embeddings]`). V0.1+.
    ///
    /// `None` when the section is absent — `vestige init` does not emit it.
    /// Presence enables semantic/hybrid search; absence falls back to lexical FTS.
    #[serde(default)]
    pub embeddings: Option<EmbeddingsConfigSection>,

    /// Optional search-mode defaults (TOML `[search]`). V0.1+.
    ///
    /// `None` when the section is absent — existing V0 configs stay unaffected.
    #[serde(default)]
    pub search: Option<SearchConfigSection>,
}

// === SECTION STRUCTS ===

/// Configuration for the embedding provider (`[embeddings]` in `.vestige/config.toml`).
///
/// All fields are optional — omitting the section keeps behaviour identical to V0
/// (lexical FTS only, `"fake"` provider for tests).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct EmbeddingsConfigSection {
    /// Embedding backend. `"fake"` | `"fastembed"` | `"ollama"`. Default: `"fake"`.
    ///
    /// `"fake"` works out of the box and is suitable for tests. Switch to
    /// `"fastembed"` for real semantic recall after installing with
    /// `cargo build --features fastembed`.
    pub provider: Option<String>,

    /// Model identifier passed to the provider.
    ///
    /// Omit to use the provider's recommended default (e.g. `bge-small-en-v1.5`
    /// for fastembed). Set explicitly to pin a specific model version.
    pub model: Option<String>,

    /// Vector dimensionality. Omit to use the provider's native dimensions.
    ///
    /// Must match the dimensions of any existing embeddings in the store — changing
    /// this on an existing project requires a full re-embed (`vestige embed --all`).
    pub dimensions: Option<usize>,

    /// Which representation kinds to embed. PRD §6.2 recommends `["summary", "compressed"]`.
    ///
    /// Omit to use the provider default. Values are matched against
    /// `MemoryRepresentationKind` string names.
    pub default_representations: Option<Vec<String>>,
}

/// Borrow conversion: section → runtime [`EmbeddingsConfig`].
///
/// Used by every code path that needs to build an embedding provider from a
/// loaded `.vestige/config.toml`. The CLI and MCP layers used to carry their
/// own copies of this mapping; this `From` impl is the single source of truth.
impl From<&EmbeddingsConfigSection> for EmbeddingsConfig {
    fn from(section: &EmbeddingsConfigSection) -> Self {
        EmbeddingsConfig {
            provider: section
                .provider
                .clone()
                .unwrap_or_else(|| DEFAULT_PROVIDER.into()),
            model: section.model.clone(),
            dimensions: section.dimensions,
        }
    }
}

/// Build an [`EmbeddingsConfig`] from an optional section, defaulting to
/// [`DEFAULT_PROVIDER`] when absent.
///
/// Provided as a free function (not a `From<Option<_>> for EmbeddingsConfig`
/// impl) because Rust's orphan rules block trait impls when both `Option` and
/// `EmbeddingsConfig` are foreign types.
pub fn embeddings_config_for(section: Option<&EmbeddingsConfigSection>) -> EmbeddingsConfig {
    match section {
        Some(s) => s.into(),
        None => EmbeddingsConfig {
            provider: DEFAULT_PROVIDER.into(),
            model: None,
            dimensions: None,
        },
    }
}

/// Configuration for search behaviour (`[search]` in `.vestige/config.toml`).
///
/// Controls the fallback search mode when no explicit `--mode` flag is passed.
/// See `vestige_core::memory::search::resolve_default_mode` for the full
/// precedence chain (explicit flag → config default → `"lexical"`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SearchConfigSection {
    /// Default retrieval strategy. `"lexical"` | `"semantic"` | `"hybrid"`.
    ///
    /// Defaults to `"lexical"` when absent (backwards-compatible with V0).
    /// Set to `"hybrid"` once embeddings are configured for best recall quality.
    pub default_mode: Option<String>,
}

/// Storage location for the SQLite database (`[storage]` in `.vestige/config.toml`).
///
/// The `path` field stores the resolved `~/.vestige/projects/<id>/memory.sqlite`
/// path using `~` notation. Call [`VestigeConfig::resolved_storage_path`] to
/// expand it to an absolute [`PathBuf`](std::path::PathBuf).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    /// Storage strategy. Always `"user_data"` in V0.
    ///
    /// `"user_data"` means the DB lives in `~/.vestige/projects/` — outside the
    /// repo and never committed. Alternative modes are reserved for future use.
    pub mode: String,

    /// `~`-prefixed path to `memory.sqlite`.
    ///
    /// Written at `vestige init` time by [`build_init_config`](crate::build_init_config).
    /// Expand with [`VestigeConfig::resolved_storage_path`] before use.
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

/// Recall and search defaults (`[recall]` in `.vestige/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallConfig {
    /// Representation depth returned by `vestige context` and `vestige search`.
    ///
    /// `"one_liner"` | `"summary"` | `"compressed"` | `"full"`.
    /// Default: `"one_liner"` (token-efficient; agents request deeper on demand).
    pub default_depth: String,

    /// Maximum results returned per search or context query. Default: `8`.
    ///
    /// Agents should page via `--limit`/`--offset` rather than raising this cap.
    pub max_results: u32,

    /// Whether to merge global (cross-project) preferences into context output.
    ///
    /// Always `false` in V0 — cross-project recall is deferred to V0.7.
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

/// MCP capability gates (`[mcp]` in `.vestige/config.toml`).
///
/// Each flag controls whether the corresponding MCP tool is enabled.
/// Disable tools you don't want an agent to invoke autonomously.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpConfig {
    /// Permit the `vestige_remember` MCP tool for observation-type memories. Default: `true`.
    pub allow_record_observation: bool,

    /// Permit the `vestige_remember` MCP tool for decision-type memories. Default: `true`.
    pub allow_record_decision: bool,

    /// Permit the `vestige_forget` MCP tool (soft-delete). Default: `false`.
    ///
    /// Off by default — destructive action requires explicit opt-in. The underlying
    /// operation is always a soft-delete; no data is permanently removed.
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

// === METHODS ===

impl VestigeConfig {
    /// Parse `project_id` into a typed [`ProjectId`].
    ///
    /// Returns [`ConfigError::InvalidProjectId`] if the stored string does not
    /// carry the required `proj_` prefix — this typically means the config was
    /// hand-edited or produced by a pre-prefix build.
    pub fn project_id(&self) -> Result<ProjectId> {
        use std::str::FromStr;
        ProjectId::from_str(&self.project_id)
            .map_err(|e| ConfigError::InvalidProjectId(e.to_string()))
    }

    /// Expand `storage.path` from `~`-notation to an absolute [`PathBuf`](std::path::PathBuf).
    ///
    /// Delegates to the internal `expand_tilde` helper in `vestige_config::paths`.
    /// Returns [`ConfigError::NoHome`] if `$HOME` cannot be resolved.
    pub fn resolved_storage_path(&self) -> Result<std::path::PathBuf> {
        crate::paths::expand_tilde(&self.storage.path)
    }
}
