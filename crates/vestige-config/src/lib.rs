//! `.vestige/config.toml` loader and project identity resolution (PRD §9.3, §10).
//!
//! Project ID resolution order:
//!   1. Explicit `--name` (slugified) — when provided to `vestige init`.
//!   2. Hash of `git remote get-url origin` if available.
//!   3. Hash of git root absolute path (or cwd if no git).
//!
//! The resolved id is stable for the same repo on the same machine and is
//! persisted to `.vestige/config.toml` so subsequent reads skip the resolver.

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
    EmbeddingsConfigSection, McpConfig, RecallConfig, SearchConfigSection, StorageConfig,
    VestigeConfig,
};

pub const CONFIG_DIR: &str = ".vestige";
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml deserialize: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("home directory not found")]
    NoHome,

    #[error("no .vestige/config.toml found from {0:?}")]
    NotFound(std::path::PathBuf),

    #[error("invalid project id in config: {0}")]
    InvalidProjectId(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;
