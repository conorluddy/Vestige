//! Atomic JSON status-file writer for the daemon's read-only observability surface.
//!
//! `~/.vestige/daemon.status.json` is rewritten every ~5 seconds by the Wave 3
//! scheduler. Observers (`vestige daemon status`, the Swift menu-bar app) read
//! this file without taking any lock. The write is therefore always atomic:
//! we write to a `.tmp` sibling then POSIX-`rename` it into place. POSIX
//! `rename(2)` is atomic on the same filesystem — readers never see a partial
//! or truncated file.
//!
//! # Schema stability
//!
//! [`DaemonStatus`] and its nested types are the contract for the Swift app.
//! Once V0.5 lands, **evolve additively only** — new fields are fine, removing
//! or renaming existing ones is a breaking change.
//!
//! # Threading model
//!
//! This module exposes pure functions. No `Mutex`, no shared state. The Wave 3
//! scheduler holds the current [`DaemonStatus`] value and calls [`write_atomic`]
//! on its own timer tick.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use vestige_core::ProjectId;

use crate::errors::DaemonError;

// === TYPES ===

/// Snapshot of the running daemon, written atomically to
/// `~/.vestige/daemon.status.json` every ~5 seconds.
///
/// The full set of fields is the contract for the Swift menu-bar app.
/// Evolve additively: add fields freely, never remove or rename existing ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Schema version. Start at 1; bump only on breaking changes.
    pub schema_version: u32,
    /// `vestige` crate version at build time (`CARGO_PKG_VERSION`).
    pub version: String,
    /// OS PID of the running daemon process.
    pub pid: u32,
    /// RFC3339 timestamp when this daemon process started.
    pub started_at: String,
    /// Whole-second uptime since `started_at`.
    pub uptime_secs: u64,
    /// One entry per project the daemon has discovered or been pinged about.
    pub projects: Vec<ProjectStatus>,
    /// Next-scheduled jobs across all projects, ordered by `at` ascending.
    pub next_jobs: Vec<ScheduledJob>,
}

/// Per-project observability data included in every status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatus {
    /// Validated project ID (`proj_<slug-or-ULID>`).
    pub project_id: ProjectId,
    /// Human-readable project name from `.vestige/config.toml`.
    pub project_name: String,
    /// Absolute path to the repository root.
    pub repo_root: String,
    /// RFC3339 timestamp of the last embed sweep, or `None` if never run.
    pub last_embed_run: Option<String>,
    /// RFC3339 timestamp of the last trace-prune job, or `None` if never run.
    pub last_prune_run: Option<String>,
    /// RFC3339 timestamp of the last candidate-TTL sweep, or `None` if never run.
    pub last_ttl_run: Option<String>,
    /// Count of memory representations that still need embedding.
    pub pending_embeds: u64,
}

/// A scheduled job entry, used in [`DaemonStatus::next_jobs`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// The class of background work to be done.
    pub kind: JobKind,
    /// The project this job targets, or `None` when the job sweeps all projects.
    ///
    /// All three daemon sweep jobs (embed / prune / TTL) run across every registered
    /// project in a single tick, so `project_id` is `None` here. The field is
    /// `Option<ProjectId>` rather than a `"*"` sentinel because `ProjectId`
    /// validates the `proj_` prefix and `"*"` would violate it. Additive serde
    /// change: `skip_serializing_if` suppresses the field when absent so the Swift
    /// app contract (which ignores unknown/absent fields) is not broken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    /// RFC3339 timestamp of the next scheduled execution.
    pub at: String,
}

/// The classes of background work the daemon schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Sweep unembedded memory representations through the configured provider.
    Embed,
    /// Evict old query-trace events to keep the trace table bounded.
    Prune,
    /// Mark overdue assimilation candidates as rejected via TTL policy.
    CandidateTtl,
}

// === PUBLIC API ===

/// Resolve the status-file path, honouring an optional per-test override.
///
/// Default: `~/.vestige/daemon.status.json`.
/// Pass [`DaemonOpts::status_file`] here to keep test state isolated in a
/// `tempfile::TempDir`.
pub fn resolve_status_path(override_path: Option<&Path>) -> PathBuf {
    if let Some(path) = override_path {
        return path.to_path_buf();
    }
    default_vestige_dir().join("daemon.status.json")
}

/// Atomically write `status` as pretty-printed JSON to `path`.
///
/// Write flow:
/// 1. Ensure the parent directory exists (creates it if needed).
/// 2. Build a `.tmp` sibling path.
/// 3. Serialize `status` as pretty-printed JSON into the `.tmp` file.
/// 4. Call `sync_all` so bytes hit disk before the rename.
/// 5. POSIX-`rename` the `.tmp` file over `path`. On the same filesystem
///    this is atomic — observers never see a partial write.
///
/// On any error, attempts best-effort cleanup of the `.tmp` file before
/// returning the error.
pub fn write_atomic(path: &Path, status: &DaemonStatus) -> Result<(), DaemonError> {
    ensure_parent_dir_exists(path)?;

    let tmp_path = tmp_path_for(path);

    if let Err(err) = write_json_to_tmp(&tmp_path, status) {
        fs::remove_file(&tmp_path).ok();
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        fs::remove_file(&tmp_path).ok();
        return Err(DaemonError::Io(err));
    }

    Ok(())
}

/// Read the status file if it exists.
///
/// Returns `Ok(None)` when the file is absent — the expected state when the
/// daemon is not running. Returns an error only for I/O or parse failures on
/// a file that is present.
pub fn read(path: &Path) -> Result<Option<DaemonStatus>, DaemonError> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let status = serde_json::from_str::<DaemonStatus>(&contents)
                .map_err(|err| DaemonError::Io(std::io::Error::other(err.to_string())))?;
            Ok(Some(status))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(DaemonError::Io(err)),
    }
}

// === PRIVATE HELPERS ===

/// Build the `.tmp` sibling path used during atomic writes.
///
/// If `path` ends with `.json` the result ends with `.json.tmp`.
/// Otherwise `.tmp` is appended directly — covers edge-case override paths
/// used in tests.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let new_extension = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => "json.tmp".to_string(),
        Some(existing) => format!("{existing}.tmp"),
        None => "tmp".to_string(),
    };
    tmp.set_extension(new_extension);
    tmp
}

/// Create all ancestor directories of `path` if they do not already exist.
fn ensure_parent_dir_exists(path: &Path) -> Result<(), DaemonError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Serialize `status` into `tmp_path` as pretty-printed JSON, then `sync_all`.
fn write_json_to_tmp(tmp_path: &Path, status: &DaemonStatus) -> Result<(), DaemonError> {
    let file = File::create(tmp_path)?;
    serde_json::to_writer_pretty(&file, status)
        .map_err(|err| DaemonError::Io(std::io::Error::other(err.to_string())))?;
    file.sync_all()?;
    Ok(())
}

/// Return `~/.vestige`, falling back to `$HOME/.vestige` in minimal environments.
fn default_vestige_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().join(".vestige"))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".vestige")
        })
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::ProjectId;

    /// Build a minimal [`DaemonStatus`] for use in tests.
    fn minimal_status() -> DaemonStatus {
        DaemonStatus {
            schema_version: 1,
            version: "0.5.0".to_string(),
            pid: std::process::id(),
            started_at: "2026-05-19T12:00:00Z".to_string(),
            uptime_secs: 0,
            projects: Vec::new(),
            next_jobs: Vec::new(),
        }
    }

    #[test]
    fn write_atomic_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let status_path = dir.path().join("nested").join("dir").join("status.json");

        write_atomic(&status_path, &minimal_status())
            .expect("write_atomic should create nested parent dirs");

        assert!(status_path.exists(), "status file must exist after write");

        let raw = fs::read_to_string(&status_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(parsed.is_object(), "file must contain valid JSON object");
    }

    #[test]
    fn write_atomic_replaces_existing() {
        let dir = TempDir::new().unwrap();
        let status_path = dir.path().join("status.json");

        let mut status_a = minimal_status();
        status_a.uptime_secs = 10;
        write_atomic(&status_path, &status_a).unwrap();

        let mut status_b = minimal_status();
        status_b.uptime_secs = 99;
        write_atomic(&status_path, &status_b).unwrap();

        let result = read(&status_path).unwrap().expect("must have a status");
        assert_eq!(result.uptime_secs, 99, "second write must replace first");

        // No leftover .tmp file.
        let tmp = tmp_path_for(&status_path);
        assert!(
            !tmp.exists(),
            ".tmp file must not exist after successful write"
        );
    }

    #[test]
    fn write_atomic_leaves_no_tmp_on_success() {
        let dir = TempDir::new().unwrap();
        let status_path = dir.path().join("status.json");

        write_atomic(&status_path, &minimal_status()).unwrap();

        let tmp = tmp_path_for(&status_path);
        assert!(
            !tmp.exists(),
            ".tmp sibling must be removed after successful atomic rename"
        );
    }

    #[test]
    fn read_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let status_path = dir.path().join("status.json");

        let result = read(&status_path).expect("missing file should be Ok(None), not Err");
        assert!(result.is_none(), "absent file must return Ok(None)");
    }

    #[test]
    fn round_trip_preserves_fields() {
        let dir = TempDir::new().unwrap();
        let status_path = dir.path().join("status.json");

        let status = DaemonStatus {
            schema_version: 1,
            version: "0.5.0-test".to_string(),
            pid: 42,
            started_at: "2026-05-19T09:00:00Z".to_string(),
            uptime_secs: 7200,
            projects: vec![
                ProjectStatus {
                    project_id: ProjectId::from_slug("alpha"),
                    project_name: "Alpha Project".to_string(),
                    repo_root: "/Users/test/alpha".to_string(),
                    last_embed_run: Some("2026-05-19T10:00:00Z".to_string()),
                    last_prune_run: None,
                    last_ttl_run: Some("2026-05-19T08:00:00Z".to_string()),
                    pending_embeds: 3,
                },
                ProjectStatus {
                    project_id: ProjectId::from_slug("beta"),
                    project_name: "Beta Project".to_string(),
                    repo_root: "/Users/test/beta".to_string(),
                    last_embed_run: None,
                    last_prune_run: Some("2026-05-18T22:00:00Z".to_string()),
                    last_ttl_run: None,
                    pending_embeds: 0,
                },
            ],
            next_jobs: vec![
                ScheduledJob {
                    kind: JobKind::Embed,
                    project_id: Some(ProjectId::from_slug("alpha")),
                    at: "2026-05-19T10:10:00Z".to_string(),
                },
                ScheduledJob {
                    kind: JobKind::Prune,
                    project_id: Some(ProjectId::from_slug("alpha")),
                    at: "2026-05-20T09:00:00Z".to_string(),
                },
                ScheduledJob {
                    kind: JobKind::CandidateTtl,
                    project_id: Some(ProjectId::from_slug("beta")),
                    at: "2026-05-19T11:00:00Z".to_string(),
                },
            ],
        };

        write_atomic(&status_path, &status).unwrap();
        let restored = read(&status_path)
            .unwrap()
            .expect("must have status after write");

        assert_eq!(restored.schema_version, 1);
        assert_eq!(restored.version, "0.5.0-test");
        assert_eq!(restored.pid, 42);
        assert_eq!(restored.started_at, "2026-05-19T09:00:00Z");
        assert_eq!(restored.uptime_secs, 7200);

        assert_eq!(restored.projects.len(), 2);
        assert_eq!(restored.projects[0].project_id.as_str(), "proj_alpha");
        assert_eq!(restored.projects[0].project_name, "Alpha Project");
        assert_eq!(restored.projects[0].pending_embeds, 3);
        assert_eq!(
            restored.projects[0].last_embed_run.as_deref(),
            Some("2026-05-19T10:00:00Z")
        );
        assert!(restored.projects[0].last_prune_run.is_none());
        assert_eq!(
            restored.projects[0].last_ttl_run.as_deref(),
            Some("2026-05-19T08:00:00Z")
        );
        assert_eq!(restored.projects[1].project_id.as_str(), "proj_beta");
        assert!(restored.projects[1].last_embed_run.is_none());
        assert_eq!(
            restored.projects[1].last_prune_run.as_deref(),
            Some("2026-05-18T22:00:00Z")
        );

        assert_eq!(restored.next_jobs.len(), 3);
        assert_eq!(restored.next_jobs[0].kind, JobKind::Embed);
        assert_eq!(
            restored.next_jobs[0]
                .project_id
                .as_ref()
                .map(|p| p.as_str()),
            Some("proj_alpha")
        );
        assert_eq!(restored.next_jobs[1].kind, JobKind::Prune);
        assert_eq!(restored.next_jobs[2].kind, JobKind::CandidateTtl);
        assert_eq!(
            restored.next_jobs[2]
                .project_id
                .as_ref()
                .map(|p| p.as_str()),
            Some("proj_beta")
        );
    }
}
