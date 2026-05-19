//! Project registry — discovers and supervises per-project worker threads.
//!
//! [`ProjectRegistry`] is the single owner of all [`ProjectWorker`] handles.
//! It provides:
//!
//! - **Scan discovery**: [`ProjectRegistry::discover_and_spawn_in`] walks a
//!   `projects/` root, opens each `memory.sqlite` briefly to read the project
//!   row, then spawns a worker thread and closes the read handle.
//! - **Explicit registration**: [`ProjectRegistry::ensure_registered`] adds a
//!   single project by its coordinates (used by Wave 4 IPC).
//! - **Lookup**: [`ProjectRegistry::get`] returns a reference to a worker by
//!   `ProjectId`.
//! - **Shutdown**: [`ProjectRegistry::shutdown_all`] drains every worker
//!   gracefully.
//!
//! # Rationale for the explicit-path API
//!
//! The public [`discover_and_spawn`][ProjectRegistry::discover_and_spawn]
//! resolves `~/.vestige/projects/` from `$HOME`. Tests use the lower-level
//! [`discover_and_spawn_in`][ProjectRegistry::discover_and_spawn_in] form that
//! accepts an explicit root path — this avoids `env::set_var` races in
//! concurrent test runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use tracing::{info, warn};

use vestige_core::ProjectId;
use vestige_embed::EmbeddingProvider;
use vestige_store::Store;

use crate::errors::DaemonError;
use crate::workers::ProjectWorker;

// === TYPES ===

/// Registry of all projects the daemon is supervising.
///
/// Owns the [`ProjectWorker`] handles indexed by [`ProjectId`].
/// All mutations are synchronous — the registry lives on the tokio main task
/// and is never shared across tasks without external coordination.
pub struct ProjectRegistry {
    workers: HashMap<ProjectId, ProjectWorker>,
    busy_timeout_ms: u32,
    /// Shared embedding provider forwarded to each worker at spawn time.
    provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
}

// === PUBLIC API ===

impl ProjectRegistry {
    /// Create an empty registry.
    pub fn new(busy_timeout_ms: u32) -> Self {
        Self {
            workers: HashMap::new(),
            busy_timeout_ms,
            provider: None,
        }
    }

    /// Set the embedding provider forwarded to all subsequently spawned workers.
    ///
    /// Must be called before [`discover_and_spawn`][Self::discover_and_spawn] or
    /// [`ensure_registered`][Self::ensure_registered] if embedding is desired.
    pub fn set_provider(&mut self, provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>) {
        self.provider = provider;
    }

    /// Scan `~/.vestige/projects/*/memory.sqlite` and spawn a worker for each.
    ///
    /// This is a thin wrapper around [`discover_and_spawn_in`][Self::discover_and_spawn_in]
    /// that resolves the default projects root from `$HOME`.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Config`] if the home directory cannot be resolved.
    /// Individual per-project failures are logged as warnings and skipped rather
    /// than aborting the whole scan — one broken DB should not prevent the daemon
    /// from supervising the remaining projects.
    pub fn discover_and_spawn(&mut self) -> Result<(), DaemonError> {
        let projects_root = resolve_projects_root()?;
        self.discover_and_spawn_in(&projects_root)
    }

    /// Scan `<projects_root>/*/memory.sqlite` and spawn a worker for each found DB.
    ///
    /// This lower-level form accepts an explicit root path, making it suitable
    /// for tests that need to isolate state without mutating `$HOME`.
    ///
    /// # Per-project failure handling
    ///
    /// If a directory entry cannot be read, or the DB cannot be opened, or the
    /// `projects` table contains no row, the project is skipped with a
    /// `warn!`-level log. The function returns `Ok(())` as long as the directory
    /// scan itself succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Io`] if `projects_root` does not exist or cannot
    /// be read.
    pub fn discover_and_spawn_in(&mut self, projects_root: &Path) -> Result<(), DaemonError> {
        if !projects_root.exists() {
            info!(path = %projects_root.display(), "projects root does not exist; no projects to discover");
            return Ok(());
        }

        let entries = std::fs::read_dir(projects_root).map_err(DaemonError::Io)?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "failed to read directory entry in projects root; skipping");
                    continue;
                }
            };

            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let db_path = entry_path.join("memory.sqlite");
            if !db_path.exists() {
                continue;
            }

            match load_project_from_db(&db_path) {
                Ok(Some((project_id, project_name, repo_root))) => {
                    info!(
                        project_id = project_id.as_str(),
                        %project_name,
                        db = %db_path.display(),
                        "discovered project"
                    );
                    if let Err(e) =
                        self.spawn_and_insert(project_id, project_name, repo_root, db_path.clone())
                    {
                        warn!(
                            db = %db_path.display(),
                            error = %e,
                            "failed to spawn worker for discovered project; skipping"
                        );
                    }
                }
                Ok(None) => {
                    warn!(db = %db_path.display(), "projects table is empty in discovered DB; skipping");
                }
                Err(e) => {
                    warn!(
                        db = %db_path.display(),
                        error = %e,
                        "failed to open or query discovered DB; skipping"
                    );
                }
            }
        }

        Ok(())
    }

    /// Register a single project explicitly.
    ///
    /// Idempotent: if the project is already registered, returns `Ok(())` without
    /// spawning a second worker. Used by the Wave 4 IPC `register_project` handler.
    ///
    /// The `storage_path` is resolved from the project ID using the standard
    /// `~/.vestige/projects/<id>/memory.sqlite` layout.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::Config`] — cannot resolve the home directory for storage
    ///   path expansion.
    /// - [`DaemonError::Store`] — store fails to open (forwarded as a worker
    ///   startup error; the worker thread will exit and the first command will
    ///   surface the error).
    pub fn ensure_registered(
        &mut self,
        project_id: ProjectId,
        project_name: String,
        repo_root: PathBuf,
    ) -> Result<(), DaemonError> {
        if self.workers.contains_key(&project_id) {
            return Ok(());
        }
        let storage_path = vestige_config::storage_path_for(&project_id)?;
        self.spawn_and_insert(project_id, project_name, repo_root, storage_path)
    }

    /// Look up a worker by project ID.
    pub fn get(&self, project_id: &ProjectId) -> Option<&ProjectWorker> {
        self.workers.get(project_id)
    }

    /// Iterate over all registered project IDs.
    pub fn project_ids(&self) -> impl Iterator<Item = &ProjectId> {
        self.workers.keys()
    }

    /// Send `Shutdown` to every worker and join all threads.
    ///
    /// Consumes `self` — the registry cannot be used after this call.
    pub async fn shutdown_all(self) {
        let mut handles = Vec::with_capacity(self.workers.len());
        for (project_id, worker) in self.workers {
            handles.push((project_id, worker));
        }
        for (project_id, worker) in handles {
            if let Err(e) = worker.shutdown().await {
                warn!(
                    project_id = project_id.as_str(),
                    error = %e,
                    "error shutting down worker"
                );
            }
        }
        info!("all project workers shut down");
    }
}

// === PRIVATE HELPERS ===

/// Spawn a `ProjectWorker` for the given coordinates and insert it into the map.
impl ProjectRegistry {
    fn spawn_and_insert(
        &mut self,
        project_id: ProjectId,
        project_name: String,
        repo_root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<(), DaemonError> {
        let worker = ProjectWorker::spawn(
            project_id.clone(),
            project_name,
            repo_root,
            storage_path,
            self.busy_timeout_ms,
            self.provider.clone(),
        )?;
        self.workers.insert(project_id, worker);
        Ok(())
    }
}

/// Open `db_path` briefly, read the single `projects` row, and return
/// `(ProjectId, name, repo_root)`. Returns `Ok(None)` if the table is empty.
///
/// The `Store` is dropped before this function returns, so the worker that
/// follows can open its own connection without contention.
fn load_project_from_db(
    db_path: &Path,
) -> Result<Option<(ProjectId, String, PathBuf)>, DaemonError> {
    let store = Store::open(db_path)?;
    let info = store.project_info()?;
    drop(store);

    let (id_str, name, root_path_opt) = match info {
        Some(row) => row,
        None => return Ok(None),
    };

    let project_id = ProjectId::from_str(&id_str).map_err(|e| {
        DaemonError::Core(vestige_core::CoreError::InvalidId(format!(
            "bad project_id in {}: {e}",
            db_path.display()
        )))
    })?;

    let repo_root = root_path_opt
        .map(PathBuf::from)
        .unwrap_or_else(|| db_path.parent().unwrap_or(Path::new("/")).to_path_buf());

    Ok(Some((project_id, name, repo_root)))
}

/// Resolve `~/.vestige/projects/` using the `vestige-config` home-resolution
/// logic (checks `$HOME` then `directories::BaseDirs`).
fn resolve_projects_root() -> Result<PathBuf, DaemonError> {
    let storage = vestige_config::storage_path_for(&ProjectId::from_slug("_probe"))?;
    // storage_path_for returns ~/.vestige/projects/_probe/memory.sqlite
    // We want ~/.vestige/projects/
    let projects_root = storage
        .parent() // _probe/
        .and_then(|p| p.parent()) // projects/
        .map(|p| p.to_path_buf())
        .ok_or_else(|| {
            DaemonError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not derive projects root from storage path",
            ))
        })?;
    Ok(projects_root)
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;
    use vestige_embed::FakeEmbeddingProvider;
    use vestige_store::Store;

    /// Create a minimal DB at `path` with a seeded project row.
    fn seed_db(path: &Path, project_id: &ProjectId, name: &str, repo_root: &str) {
        let mut store = Store::open(path).expect("open store for seeding");
        store
            .ensure_project(project_id, name, Some(repo_root), None)
            .expect("seed project row");
    }

    fn fake_provider() -> Option<Arc<dyn EmbeddingProvider + Send + Sync>> {
        Some(Arc::new(FakeEmbeddingProvider::default()))
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");
        std::fs::create_dir_all(&projects_root).unwrap();

        let mut registry = ProjectRegistry::new(5000);
        registry
            .discover_and_spawn_in(&projects_root)
            .expect("discover on empty dir returns Ok");

        assert_eq!(
            registry.workers.len(),
            0,
            "no workers spawned for empty projects dir"
        );
    }

    #[test]
    fn discover_two_projects() {
        let rt = Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");

        // Project A
        let id_a = ProjectId::from_slug("aaa");
        let dir_a = projects_root.join(id_a.as_str());
        std::fs::create_dir_all(&dir_a).unwrap();
        let db_a = dir_a.join("memory.sqlite");
        seed_db(&db_a, &id_a, "Project AAA", "/repos/aaa");

        // Project B
        let id_b = ProjectId::from_slug("bbb");
        let dir_b = projects_root.join(id_b.as_str());
        std::fs::create_dir_all(&dir_b).unwrap();
        let db_b = dir_b.join("memory.sqlite");
        seed_db(&db_b, &id_b, "Project BBB", "/repos/bbb");

        let mut registry = ProjectRegistry::new(5000);
        registry.set_provider(fake_provider());
        registry
            .discover_and_spawn_in(&projects_root)
            .expect("discover returns Ok");

        assert_eq!(registry.workers.len(), 2, "two workers spawned");
        assert!(registry.get(&id_a).is_some(), "project A registered");
        assert!(registry.get(&id_b).is_some(), "project B registered");

        rt.block_on(async move {
            registry.shutdown_all().await;
        });
    }
}
