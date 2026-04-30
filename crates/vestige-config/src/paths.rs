//! TOML round-trip and filesystem path helpers.
//!
//! Three responsibilities, kept together because they all touch the filesystem:
//!
//! 1. **Config discovery** — walk from a starting directory up to the root
//!    looking for `.vestige/config.toml` ([`discover_config`]).
//! 2. **Config read/write** — deserialise from and serialise to TOML
//!    ([`read_config`], [`write_config`]).
//! 3. **Storage path resolution** — derive the user-local SQLite path
//!    `~/.vestige/projects/<id>/memory.sqlite` ([`storage_path_for`]).
//!
//! # In-repo vs. user-local paths
//!
//! `.vestige/config.toml` lives **inside the repo** and is committed. It contains
//! only the project pin and capability flags — no private data.
//!
//! `~/.vestige/projects/<project_id>/memory.sqlite` lives **outside the repo**
//! on the user's machine and is never committed. It holds the full memory journal.

use std::path::{Path, PathBuf};

use tracing::debug;

use vestige_core::ProjectId;

use crate::schema::VestigeConfig;
use crate::{ConfigError, Result, CONFIG_DIR, CONFIG_FILE};

// === PUBLIC API ===

/// Locate `.vestige/config.toml` by walking from `start` up to the filesystem root.
///
/// Returns the canonical path to the first `config.toml` found and the parsed
/// [`VestigeConfig`]. The search mirrors how `git` locates `.git` — it works
/// from any subdirectory within the project.
///
/// # Errors
///
/// Returns [`ConfigError::NotFound`] (containing `start`) if no config file is
/// found before reaching the filesystem root.
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

/// Read and deserialise a [`VestigeConfig`] from the TOML file at `path`.
///
/// # Errors
///
/// - [`ConfigError::Io`] — file not found or not readable.
/// - [`ConfigError::TomlDe`] — the file is not valid TOML or does not match
///   the [`VestigeConfig`] schema.
pub fn read_config(path: &Path) -> Result<VestigeConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: VestigeConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

/// Serialise `cfg` to pretty TOML and write it to `path`.
///
/// Parent directories are created with `create_dir_all` if they don't exist,
/// so callers can pass a path inside a `.vestige/` directory that hasn't been
/// created yet.
///
/// # Errors
///
/// - [`ConfigError::TomlSer`] — serialisation failed (should not happen for
///   well-formed [`VestigeConfig`] values).
/// - [`ConfigError::Io`] — write failed (permissions, disk full, etc.).
pub fn write_config(path: &Path, cfg: &VestigeConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml = toml::to_string_pretty(cfg)?;
    std::fs::write(path, toml)?;
    debug!(?path, "wrote config");
    Ok(())
}

/// Resolve the SQLite storage path for a project.
///
/// Always returns `~/.vestige/projects/<project_id>/memory.sqlite` expanded to an
/// absolute path. The home directory is resolved from `$HOME`, falling back to
/// `directories::BaseDirs` on platforms where `HOME` may be absent.
///
/// The parent directory is **not** created here — the store layer creates it on
/// first open.
///
/// # Errors
///
/// Returns [`ConfigError::NoHome`] if the home directory cannot be determined
/// (e.g. `$HOME` is unset in a restricted environment).
pub fn storage_path_for(project_id: &ProjectId) -> Result<PathBuf> {
    let home = home_dir().ok_or(ConfigError::NoHome)?;
    Ok(home
        .join(".vestige")
        .join("projects")
        .join(project_id.as_str())
        .join("memory.sqlite"))
}

// === PRIVATE HELPERS ===

/// Expand a `~`-prefixed path string to an absolute [`PathBuf`].
///
/// - `"~/foo/bar"` → `<home>/foo/bar`
/// - `"~"` → `<home>`
/// - Anything else → treated as a literal path (no expansion).
///
/// # Errors
///
/// Returns [`ConfigError::NoHome`] if `~` is present but the home directory
/// cannot be resolved (e.g. `$HOME` unset and `directories::BaseDirs` fails).
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

/// Serialise a path back to a string, replacing the home prefix with `~`.
///
/// Produces compact, portable representations like `~/.vestige/projects/…`
/// suitable for storing in `config.toml`. If the home directory cannot be
/// determined, the absolute path is returned unchanged.
pub(crate) fn stringify_path_with_tilde(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

/// Resolve the current user's home directory.
///
/// Checks `$HOME` first; falls back to `directories::BaseDirs` for platforms
/// where `HOME` may not be set.
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
