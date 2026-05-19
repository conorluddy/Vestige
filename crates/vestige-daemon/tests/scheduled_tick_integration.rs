//! Wave 8.6 — prove the scheduler's tokio interval timers actually fire.
//!
//! All other tests in the suite use explicit kicks (`daemon.kick` IPC or direct
//! `embed_sweep::run_once` calls). A bug in the `tokio::select!` interval arms
//! wouldn't be caught by those tests. This test seeds unembedded memories,
//! starts the daemon with a very short embed-sweep cadence, and waits long
//! enough for the timer to fire — without ever sending a kick.

use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::watch;
use vestige_config::{ResolvedDaemonConfig, DAEMON_DEFAULT_CANDIDATE_TTL_DAYS};
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId};
use vestige_daemon::{run_with_cancel, DaemonOpts};
use vestige_store::Store;

// === HELPERS ===

/// Open (or create) a store at `<dir>/memory.sqlite`.
fn open_store(dir: &std::path::Path) -> Store {
    Store::open(dir.join("memory.sqlite")).expect("open store")
}

/// Seed a project row and one memory into the store.
fn seed_project_with_memory(project_dir: &std::path::Path, project_id: &ProjectId) {
    let mut store = open_store(project_dir);

    store
        .ensure_project(
            project_id,
            "Scheduled Tick Test Project",
            Some("/tmp/tick-test-repo"),
            None,
        )
        .expect("ensure_project");

    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: MemoryType::Note,
            body: "Wave 8.6: embed timer fires without a kick.",
            importance: 0.5,
            source: None,
        },
    )
    .expect("build_bundle");

    store.record_memory(&bundle).expect("record_memory");
}

// === TEST ===

/// The scheduler's embed interval timer fires on its own — no kick required.
///
/// Seeds one memory, starts the daemon with a 2-second embed-sweep cadence,
/// waits 4 seconds (2× cadence for CI headroom), then asserts that at least
/// one embedding was written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_sweep_fires_on_schedule_without_kick() {
    // 1. Isolate all file I/O inside a TempDir.
    let home = TempDir::new().unwrap();
    let projects_root = home.path().join("projects");
    let project_id = ProjectId::from_slug("tick");
    let project_dir = projects_root.join(project_id.as_str());
    let memory_db = project_dir.join("memory.sqlite");
    std::fs::create_dir_all(&project_dir).unwrap();

    // 2. Seed project + memory so there's something for the embed sweep to pick up.
    seed_project_with_memory(&project_dir, &project_id);

    // Confirm the memory exists and has no embeddings yet.
    {
        let store = Store::open(&memory_db).unwrap();
        let status = store
            .embedding_status(&project_id)
            .expect("embedding_status");
        assert_eq!(status.total_active_memories, 1, "one memory seeded");
        assert_eq!(
            status.embedded_representations, 0,
            "no embeddings before daemon run"
        );
    }

    // 3. Build a ResolvedDaemonConfig with a very short embed-sweep cadence.
    //    All other sweeps use 24-hour intervals so only the embed timer fires.
    let cfg = ResolvedDaemonConfig {
        enabled: true,
        embed_sweep_interval_secs: 2,
        trace_prune_interval_secs: 86_400,
        candidate_ttl_sweep_interval_secs: 86_400,
        candidate_ttl_days: DAEMON_DEFAULT_CANDIDATE_TTL_DAYS,
        log_level: "info".into(),
        socket_path: Some(
            home.path()
                .join("daemon.sock")
                .to_string_lossy()
                .into_owned(),
        ),
        status_file_path: Some(
            home.path()
                .join("daemon.status.json")
                .to_string_lossy()
                .into_owned(),
        ),
    };

    // 4. Build DaemonOpts pointing at the TempDir, with config_override = Some(cfg).
    //    The socket and status paths in opts override the ones inside cfg so the
    //    daemon writes to the same TempDir and doesn't touch ~/.vestige.
    let opts = DaemonOpts {
        foreground: true,
        pid_file: Some(home.path().join("daemon.pid")),
        socket_path: Some(home.path().join("daemon.sock")),
        status_file: Some(home.path().join("daemon.status.json")),
        log_file: None,
        projects_root: Some(projects_root.clone()),
        config_override: Some(cfg),
    };

    // 5. Spawn run_with_cancel in a tokio task; retain the cancel sender.
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let daemon_task = tokio::spawn(async move { run_with_cancel(opts, cancel_rx).await });

    // 6. Wait 4 seconds — the embed tick skips t=0 then fires at t≈2 s.
    //    4 s gives 2× headroom for slow CI machines.
    tokio::time::sleep(Duration::from_secs(4)).await;

    // 7. Cancel the daemon and wait for it to stop cleanly.
    cancel_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), daemon_task)
        .await
        .expect("daemon should shut down within 5 s after cancel")
        .expect("daemon task should not panic")
        .expect("run_with_cancel should return Ok");

    // 8. Open the store (daemon has shut down, so no WAL contention) and assert
    //    that the embed sweep wrote at least one embedding.
    let store = Store::open(&memory_db).unwrap();
    let status = store
        .embedding_status(&project_id)
        .expect("embedding_status after daemon run");

    assert!(
        status.embedded_representations >= 1,
        "expected embed_tick to have written ≥1 embedding without a kick; \
         embedded_representations={}",
        status.embedded_representations
    );
}
