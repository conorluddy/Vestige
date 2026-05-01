//! `.vestige/config.toml` loader and project identity resolution (PRD §9.3, §10).
//!
//! # Responsibilities
//!
//! - Parse and round-trip `.vestige/config.toml` (in-repo, committed, no private data).
//! - Resolve a stable `ProjectId` for the current repo using the PRD §9.3 precedence chain.
//! - Derive the user-local storage path `~/.vestige/projects/<id>/memory.sqlite`.
//!
//! # Project ID resolution order (PRD §9.3)
//!
//! 1. Explicit `--name` passed to `vestige init` — slugified to `proj_<slug>`.
//! 2. SHA-256 of `git remote get-url origin` — stable across machines with the same remote.
//! 3. SHA-256 of the absolute repo-root path — fallback for repos without a remote.
//!
//! The resolved id is written into `.vestige/config.toml` so subsequent invocations
//! read it directly without re-running the resolver.
//!
//! # Storage split
//!
//! | Location | Committed? | Content |
//! |---|---|---|
//! | `.vestige/config.toml` (in repo) | yes | project pin, scope, behaviour flags |
//! | `~/.vestige/projects/<id>/memory.sqlite` | never | private memory journal |
//!
//! Callers outside this crate use [`CONFIG_DIR`] / [`CONFIG_FILE`] rather than
//! hardcoding `.vestige` strings.

use thiserror::Error;

pub mod identity;
pub mod paths;
pub mod schema;

pub use identity::{
    build_init_config, discover_repo_root, display_name_from_path, git_remote_url,
    resolve_project_id,
};
pub use paths::{discover_config, read_config, storage_path_for, write_config};
pub use schema::{
    embeddings_config_for, EmbeddingsConfigSection, McpConfig, RecallConfig, SearchConfigSection,
    StorageConfig, VestigeConfig,
};

/// The in-repo config directory name. Always `.vestige`.
///
/// Every path referencing the config directory must use this constant — never
/// hardcode the string literal elsewhere in the workspace.
pub const CONFIG_DIR: &str = ".vestige";

/// The config filename inside [`CONFIG_DIR`]. Always `config.toml`.
pub const CONFIG_FILE: &str = "config.toml";

/// Errors returned by all `vestige-config` operations.
///
/// Variants map to the four failure domains: filesystem I/O, TOML
/// (de)serialisation, missing home directory, and invalid project ids loaded
/// from a corrupt or hand-edited config file.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A filesystem operation (read, write, create-dir) failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The TOML source could not be deserialized into [`VestigeConfig`].
    #[error("toml deserialize: {0}")]
    TomlDe(#[from] toml::de::Error),

    /// [`VestigeConfig`] could not be serialized to TOML.
    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),

    /// `$HOME` is unset and `directories::BaseDirs` could not locate the home
    /// directory. Path resolution requiring `~` expansion will fail.
    #[error("home directory not found")]
    NoHome,

    /// No `.vestige/config.toml` was found by walking from the given path to
    /// the filesystem root. The inner [`PathBuf`](std::path::PathBuf) is the
    /// directory the search started from.
    #[error("no .vestige/config.toml found from {0:?}")]
    NotFound(std::path::PathBuf),

    /// The `project_id` field in a config file could not be parsed as a valid
    /// [`ProjectId`](vestige_core::ProjectId). Likely a hand-edit or a V0-era
    /// config written before the `proj_` prefix was enforced.
    #[error("invalid project id in config: {0}")]
    InvalidProjectId(String),
}

/// Convenience alias — all fallible functions in this crate return `Result<T>`.
pub type Result<T> = std::result::Result<T, ConfigError>;
