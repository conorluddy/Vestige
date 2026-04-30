//! TOML round-trip and filesystem path helpers.
//! Covers config discovery, read/write, storage path resolution, and tilde expansion.

use std::path::{Path, PathBuf};

use tracing::debug;

use vestige_core::ProjectId;

use crate::schema::VestigeConfig;
use crate::{ConfigError, Result, CONFIG_DIR, CONFIG_FILE};

/// Locate a `.vestige/config.toml` by walking from `start` up to filesystem root.
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

/// Read and deserialize a config file at `path`.
pub fn read_config(path: &Path) -> Result<VestigeConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: VestigeConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

/// Serialize `cfg` and write it to `path`, creating parent directories as needed.
pub fn write_config(path: &Path, cfg: &VestigeConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml = toml::to_string_pretty(cfg)?;
    std::fs::write(path, toml)?;
    debug!(?path, "wrote config");
    Ok(())
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

pub(crate) fn expand_tilde(s: &str) -> Result<PathBuf> {
    if let Some(rest) = s.strip_prefix("~/") {
        let home = home_dir().ok_or(ConfigError::NoHome)?;
        Ok(home.join(rest))
    } else if s == "~" {
        home_dir().ok_or(ConfigError::NoHome)
    } else {
        Ok(PathBuf::from(s))
    }
}

pub(crate) fn stringify_path_with_tilde(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::ProjectId;

    use crate::identity::build_init_config;

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
