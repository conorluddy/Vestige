//! Wave 3 acceptance test: scheduler tick → worker embed → embeddings exist.
//!
//! Seeds a project DB with memories, runs an embed sweep via the registry, and
//! asserts that embeddings were written to the store.

use std::sync::Arc;

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

use vestige_daemon::{jobs::embed_sweep, registry::ProjectRegistry};

// === HELPERS ===

/// Open a fresh store at `<dir>/memory.sqlite`.
fn open_store(dir: &std::path::Path) -> Store {
    Store::open(dir.join("memory.sqlite")).expect("open store")
}

/// Seed a project row.
fn seed_project(store: &mut Store, project_id: &ProjectId) {
    store
        .ensure_project(
            project_id,
            "Integration Test Project",
            Some("/tmp/test-repo"),
            None,
        )
        .expect("seed project row");
}

/// Record one memory and return its ID.
fn seed_memory(store: &mut Store, project_id: &ProjectId, body: &str) -> vestige_core::MemoryId {
    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: MemoryType::Note,
            body,
            importance: 0.5,
            source: None,
        },
    )
    .expect("build bundle");
    let id = bundle.memory.id.clone();
    store.record_memory(&bundle).expect("record memory");
    id
}

// === TEST ===

/// Full path: registry discovers a project DB, worker embeds memories, embeddings exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_sweep_populates_embeddings() {
    let tmp = TempDir::new().unwrap();
    let projects_root = tmp.path().join("projects");

    // ── 1. Create the project DB layout ──────────────────────────────────────
    let project_id = ProjectId::from_slug("sweep-test");
    let project_dir = projects_root.join(project_id.as_str());
    std::fs::create_dir_all(&project_dir).unwrap();
    let db_path = project_dir.join("memory.sqlite");

    {
        let mut store = open_store(&project_dir);
        seed_project(&mut store, &project_id);
        seed_memory(
            &mut store,
            &project_id,
            "The daemon embed sweep integration test.",
        );
        seed_memory(
            &mut store,
            &project_id,
            "A second memory to ensure the sweep handles multiple.",
        );
    }

    // Confirm memories exist before the sweep.
    {
        let store = open_store(&project_dir);
        let status = store
            .embedding_status(&project_id)
            .expect("embedding_status");
        assert_eq!(status.total_active_memories, 2, "two memories seeded");
        assert_eq!(status.embedded_representations, 0, "no embeddings yet");
    }

    // ── 2. Build registry ────────────────────────────────────────────────────
    // The project's repo_root (/tmp/test-repo-*) has no .vestige/config.toml,
    // so build_project_provider falls back to FakeEmbeddingProvider automatically.
    let mut registry = ProjectRegistry::new(5000);
    registry
        .discover_and_spawn_in(&projects_root)
        .expect("discover_and_spawn_in");

    let registry = Arc::new(registry);

    // ── 3. Run one embed sweep ────────────────────────────────────────────────
    let report = embed_sweep::run_once(&registry).await;

    assert_eq!(report.projects_scanned, 1, "one project scanned");
    assert_eq!(report.projects_succeeded, 1, "one project succeeded");
    assert_eq!(report.projects_failed, 0, "no failures");
    assert!(
        report.total_embeddings_added > 0,
        "at least one embedding added; got {}",
        report.total_embeddings_added
    );

    // ── 4. Shut down registry so it releases the worker's DB handle ───────────
    // Extract the registry out of Arc (only one owner at this point).
    let registry = Arc::try_unwrap(registry)
        .unwrap_or_else(|_| panic!("registry should have exactly one owner after sweep"));
    registry.shutdown_all().await;

    // ── 5. Open the DB directly and assert embeddings were written ────────────
    let store = Store::open(&db_path).expect("open store after sweep");
    let status = store
        .embedding_status(&project_id)
        .expect("embedding_status after sweep");

    assert!(
        status.embedded_representations > 0,
        "store must contain active embeddings after sweep; got embedded_representations={}",
        status.embedded_representations
    );
}

/// Sweep on a project with no `.vestige/config.toml` succeeds via FakeEmbeddingProvider fallback.
///
/// T8.1: the daemon no longer supports a "no provider" state — every project
/// gets at least `FakeEmbeddingProvider` when config is absent. This test
/// confirms that a project whose `repo_root` has no config still embeds
/// successfully (using the fake provider) rather than counting as failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_sweep_no_config_falls_back_to_fake_and_succeeds() {
    let tmp = TempDir::new().unwrap();
    let projects_root = tmp.path().join("projects");

    let project_id = ProjectId::from_slug("no-config-sweep");
    let project_dir = projects_root.join(project_id.as_str());
    std::fs::create_dir_all(&project_dir).unwrap();

    {
        let mut store = open_store(&project_dir);
        seed_project(&mut store, &project_id);
        seed_memory(
            &mut store,
            &project_id,
            "Memory with no config, fake fallback.",
        );
    }

    // No .vestige/config.toml at repo_root (/tmp/test-repo) — build_project_provider
    // warns and falls back to FakeEmbeddingProvider.
    let mut registry = ProjectRegistry::new(5000);
    registry
        .discover_and_spawn_in(&projects_root)
        .expect("discover_and_spawn_in");

    let registry = Arc::new(registry);
    let report = embed_sweep::run_once(&registry).await;

    assert_eq!(report.projects_scanned, 1);
    assert_eq!(
        report.projects_succeeded, 1,
        "no-config project must succeed via fake fallback (T8.1)"
    );
    assert_eq!(
        report.projects_failed, 0,
        "no-config must not count as failed after T8.1"
    );

    let registry = Arc::try_unwrap(registry).unwrap_or_else(|_| panic!("single owner"));
    registry.shutdown_all().await;
}
