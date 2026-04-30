//! Project identity resolution — PRD §9.3.
//! Derives a stable `ProjectId` from explicit name, git remote, or repo path.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use vestige_core::ProjectId;

use crate::paths::stringify_path_with_tilde;
use crate::schema::{StorageConfig, VestigeConfig};

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
        recall: Default::default(),
        mcp: Default::default(),
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

/// Display name fallback: folder name. Used when `--name` not provided.
pub fn display_name_from_path(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("vestige-project")
        .to_string()
}

/// Return `origin` remote URL for the repo at `repo_root`, or `None`.
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
}
