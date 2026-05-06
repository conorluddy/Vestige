//! Project identity resolution — PRD §9.3.
//!
//! Derives a stable [`ProjectId`] from one of three sources, tried in order:
//!
//! 1. **Explicit name** — `--name` passed to `vestige init`, slugified and
//!    prefixed (`proj_my-project`). Stable as long as the name doesn't change.
//! 2. **Git remote hash** — SHA-256 of `git remote get-url origin` (first 6 bytes,
//!    hex-encoded). Stable across machines that share the same remote URL.
//! 3. **Repo-path hash** — SHA-256 of the absolute repo-root path. Stable on the
//!    same machine even without a git remote.
//!
//! The resolved id is written to `.vestige/config.toml` at `vestige init` time so
//! that subsequent invocations read it directly without re-running this logic.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use vestige_core::ProjectId;

use crate::paths::stringify_path_with_tilde;
use crate::schema::{StorageConfig, VestigeConfig};

// === PUBLIC API ===

/// Build a fresh [`VestigeConfig`] for `vestige init`.
///
/// Writes sensible defaults for all optional sections and encodes `storage_path`
/// using `~`-notation (via the internal `stringify_path_with_tilde` helper).
/// The caller is responsible for writing the result to disk with
/// [`write_config`](crate::write_config).
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
        assimilation: None,
    }
}

/// Resolve the project id for a repo following the PRD §9.3 precedence chain.
///
/// # Precedence
///
/// 1. `explicit_name` — when `Some`, slugified (non-alphanumeric replaced with `-`,
///    lowercased) and returned as `proj_<slug>`. An empty string after slugification
///    falls through to step 2.
/// 2. `git remote get-url origin` at `repo_root` — SHA-256 (first 6 bytes, hex)
///    of the remote URL. Cross-machine stable for repos that share a remote.
/// 3. Absolute path of `repo_root` — SHA-256 (first 6 bytes, hex). Machine-local
///    fallback; requires no network or git remote.
///
/// The returned id is always prefixed `proj_`.
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

/// Derive a human-readable display name from the repo root path.
///
/// Returns the directory's file-name component (e.g. `"vestige"` for
/// `/Users/alice/code/vestige`). Falls back to `"vestige-project"` if the
/// path has no file name (e.g. the filesystem root).
///
/// Used as the `project_name` default when `--name` is not supplied to
/// `vestige init`.
pub fn display_name_from_path(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("vestige-project")
        .to_string()
}

/// Return the `origin` remote URL for the git repo at `repo_root`, or `None`.
///
/// Runs `git -C <repo_root> remote get-url origin`. Returns `None` if git is not
/// available, the directory is not a git repo, or there is no `origin` remote.
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

/// Find the git repository root by walking up from `start`.
///
/// Returns the first ancestor directory containing a `.git` entry.
/// Falls back to `start` itself if no `.git` is found before the
/// filesystem root (i.e. the directory is not inside a git repo).
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

// === PRIVATE HELPERS ===

/// Convert a free-form name into a URL-safe slug.
///
/// Lowercases ASCII alphanumeric characters, replaces runs of non-alphanumeric
/// characters with a single `-`, and strips leading/trailing dashes.
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

/// Produce a short (12-character) hex string from the first 6 bytes of a SHA-256 digest.
///
/// Used as the stable suffix for project ids derived from a git remote URL or
/// repo path. 6 bytes (48 bits) gives ~281 trillion possible values — collision
/// probability is negligible for any realistic number of projects.
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
