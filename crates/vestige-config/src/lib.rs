//! `.vestige/config.toml` loader and project identity resolution (PRD §9.3, §10).
//!
//! Project ID resolution order:
//!   1. Explicit `--name` (slugified) — when provided to `vestige init`.
//!   2. Hash of `git remote get-url origin` if available.
//!   3. Hash of git root absolute path (or cwd if no git).
//!
//! The resolved id is stable for the same repo on the same machine and is
//! persisted to `.vestige/config.toml` so subsequent reads skip the resolver.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::debug;

use vestige_core::ProjectId;

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
    NotFound(PathBuf),

    #[error("invalid project id in config: {0}")]
    InvalidProjectId(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

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

fn default_scope() -> String {
    "project".into()
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
    pub fn resolved_storage_path(&self) -> Result<PathBuf> {
        expand_tilde(&self.storage.path)
    }
}

/// Locate a `.vestige/config.toml` by walking from `start` up to filesystem
/// root.
pub fn discover_config(start: &Path) -> Result<(PathBuf, VestigeConfig)> {
    let mut cursor = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        let candidate = cursor.join(CONFIG_DIR).join(CONFIG_FILE);
        if candidate.is_file() {
            let cfg = read_config(&candidate)?;
            return Ok((candidate, cfg));
        }
        if !cursor.pop() {
            return Err(ConfigError::NotFound(start.to_path_buf()));
        }
    }
}

pub fn read_config(path: &Path) -> Result<VestigeConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: VestigeConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

pub fn write_config(path: &Path, cfg: &VestigeConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml = toml::to_string_pretty(cfg)?;
    std::fs::write(path, toml)?;
    debug!(?path, "wrote config");
    Ok(())
}

/// Build a fresh config for `vestige init`.
pub fn build_init_config(
    project_id: &ProjectId,
    project_name: &str,
    storage_path: &Path,
) -> VestigeConfig {
    VestigeConfig {
        project_id: project_id.as_str().into(),
        project_name: project_name.into(),
        scope: "project".into(),
        storage: StorageConfig {
            mode: "user_data".into(),
            path: stringify_path_with_tilde(storage_path),
        },
        recall: RecallConfig::default(),
        mcp: McpConfig::default(),
        embeddings: None,
        search: None,
    }
}

/// Resolve the project id for a repo. Order matches PRD §9.3.
pub fn resolve_project_id(repo_root: &Path, explicit_name: Option<&str>) -> ProjectId {
    if let Some(name) = explicit_name {
        let slug = slugify(name);
        if !slug.is_empty() {
            return ProjectId::from_slug(slug);
        }
    }

    if let Some(remote) = git_remote_url(repo_root) {
        return ProjectId::from_slug(short_hash(remote.as_bytes()));
    }

    ProjectId::from_slug(short_hash(repo_root.to_string_lossy().as_bytes()))
}

/// Resolve the SQLite storage path for a project id.
/// PRD §9 fixes `~/.vestige/projects/<id>/memory.sqlite`.
pub fn storage_path_for(project_id: &ProjectId) -> Result<PathBuf> {
    let home = home_dir().ok_or(ConfigError::NoHome)?;
    Ok(home
        .join(".vestige")
        .join("projects")
        .join(project_id.as_str())
        .join("memory.sqlite"))
}

/// Display name fallback: folder name. Used when `--name` not provided.
pub fn display_name_from_path(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("vestige-project")
        .to_string()
}

pub fn git_remote_url(repo_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Find the git root by walking up from `start`. Falls back to `start`.
pub fn discover_repo_root(start: &Path) -> PathBuf {
    let mut cursor = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if cursor.join(".git").exists() {
            return cursor;
        }
        if !cursor.pop() {
            return start.to_path_buf();
        }
    }
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn short_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(&digest[..6])
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

fn expand_tilde(s: &str) -> Result<PathBuf> {
    if let Some(rest) = s.strip_prefix("~/") {
        let home = home_dir().ok_or(ConfigError::NoHome)?;
        Ok(home.join(rest))
    } else if s == "~" {
        home_dir().ok_or(ConfigError::NoHome)
    } else {
        Ok(PathBuf::from(s))
    }
}

fn stringify_path_with_tilde(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn slugify_strips_punctuation() {
        assert_eq!(slugify("My Project!"), "my-project");
        assert_eq!(slugify("  Spaces   here  "), "spaces-here");
        assert_eq!(slugify("Already-Slug"), "already-slug");
    }

    #[test]
    fn resolve_uses_explicit_name_first() {
        let tmp = TempDir::new().unwrap();
        let id = resolve_project_id(tmp.path(), Some("Vestige"));
        assert_eq!(id.as_str(), "proj_vestige");
    }

    #[test]
    fn resolve_falls_back_to_path_hash() {
        let tmp = TempDir::new().unwrap();
        let id1 = resolve_project_id(tmp.path(), None);
        let id2 = resolve_project_id(tmp.path(), None);
        assert_eq!(id1, id2, "ids must be stable for the same path");
        assert!(id1.as_str().starts_with("proj_"));
    }

    #[test]
    fn config_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".vestige").join("config.toml");
        let cfg = build_init_config(
            &ProjectId::from_slug("vestige"),
            "Vestige",
            Path::new("/Users/test/.vestige/projects/proj_vestige/memory.sqlite"),
        );
        write_config(&path, &cfg).unwrap();
        let read = read_config(&path).unwrap();
        assert_eq!(cfg, read);
    }

    #[test]
    fn default_config_omits_new_sections() {
        let cfg = build_init_config(
            &ProjectId::from_slug("vestige"),
            "Vestige",
            Path::new("/Users/test/.vestige/projects/proj_vestige/memory.sqlite"),
        );
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        assert!(
            !toml_str.contains("[embeddings]"),
            "default config must not emit [embeddings]"
        );
        assert!(
            !toml_str.contains("[search]"),
            "default config must not emit [search]"
        );
    }

    #[test]
    fn parses_embeddings_section() {
        let toml_str = r#"
project_id = "proj_test"
project_name = "Test"

[embeddings]
provider = "fastembed"
model = "bge-small-en-v1.5"
"#;
        let cfg: VestigeConfig = toml::from_str(toml_str).unwrap();
        let emb = cfg
            .embeddings
            .expect("embeddings section should be present");
        assert_eq!(emb.provider.as_deref(), Some("fastembed"));
        assert_eq!(emb.model.as_deref(), Some("bge-small-en-v1.5"));
        assert!(emb.dimensions.is_none());
        assert!(emb.default_representations.is_none());
    }

    #[test]
    fn parses_search_section_default_mode() {
        let toml_str = r#"
project_id = "proj_test"
project_name = "Test"

[search]
default_mode = "hybrid"
"#;
        let cfg: VestigeConfig = toml::from_str(toml_str).unwrap();
        let search = cfg.search.expect("search section should be present");
        assert_eq!(search.default_mode.as_deref(), Some("hybrid"));
    }

    #[test]
    fn existing_v0_toml_still_parses() {
        let toml_str = r#"
project_id = "proj_vestige"
project_name = "Vestige"
scope = "project"

[storage]
mode = "user_data"
path = "~/.vestige/projects/proj_vestige/memory.sqlite"

[recall]
default_depth = "one_liner"
max_results = 8
include_global_preferences = false

[mcp]
allow_record_observation = true
allow_record_decision = true
allow_forget = false
"#;
        let cfg: VestigeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.project_id, "proj_vestige");
        assert!(
            cfg.embeddings.is_none(),
            "embeddings must be None for V0 TOML"
        );
        assert!(cfg.search.is_none(), "search must be None for V0 TOML");
    }

    #[test]
    fn discover_walks_up() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("sub").join("dir");
        std::fs::create_dir_all(&nested).unwrap();
        let cfg_path = tmp.path().join(".vestige").join("config.toml");
        let cfg = build_init_config(
            &ProjectId::from_slug("x"),
            "X",
            Path::new("/tmp/x/memory.sqlite"),
        );
        write_config(&cfg_path, &cfg).unwrap();

        let (found_path, found_cfg) = discover_config(&nested).unwrap();
        assert_eq!(found_path, cfg_path.canonicalize().unwrap_or(cfg_path));
        assert_eq!(found_cfg.project_id, cfg.project_id);
    }
}
