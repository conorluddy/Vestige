//! CLI indexing honors config and never clears a working index on failure.

use serde_json::Value;
use std::process::{Command, Output};
use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

struct Repo {
    tmp: TempDir,
    store: Store,
    config: String,
}

impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".vestige")).unwrap();
        let database = tmp.path().join("memory.sqlite");
        let config = format!("project_id = \"proj_embedding-config\"\nproject_name = \"Test\"\nscope = \"project\"\n[storage]\nmode = \"user_data\"\npath = {:?}\n", database.to_str().unwrap());
        let mut store = Store::open(database).unwrap();
        let project = ProjectId::from_slug("embedding-config");
        store.ensure_project(&project, "Test", None, None).unwrap();
        store
            .record_memory(
                &build_bundle(
                    &project,
                    NewMemory {
                        r#type: MemoryType::Note,
                        body: "An indexed memory.",
                        importance: 0.5,
                        source: None,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let repo = Self { tmp, store, config };
        repo.configure("fake", 32);
        repo
    }

    fn configure(&self, provider: &str, dimensions: usize) {
        std::fs::write(
            self.tmp.path().join(".vestige/config.toml"),
            format!(
                "{}\n[embeddings]\nprovider = {provider:?}\ndimensions = {dimensions}\n",
                self.config
            ),
        )
        .unwrap();
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vestige"))
            .current_dir(self.tmp.path())
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn vectors(&self) -> Vec<(String, Vec<u8>)> {
        self.store
            .connection()
            .prepare("SELECT embedding_id, vector FROM memory_vectors ORDER BY embedding_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}

#[test]
fn embed_and_reindex_honor_project_dimensions_and_cli_provider_overrides() {
    let repo = Repo::new();
    assert_eq!(repo.json(&["embed", "--all", "--json"])["dimensions"], 32);
    assert_eq!(
        repo.json(&["reindex", "--embeddings", "--json"])["embeddings"]["dimensions"],
        32
    );
    assert_eq!(
        repo.json(&["embeddings", "status", "--json"])["dimensions"],
        32
    );
    repo.configure("unknown-provider", 128);
    assert_eq!(
        repo.json(&["embed", "--all", "--provider", "fake", "--json"])["dimensions"],
        64
    );
    assert_eq!(
        repo.json(&["reindex", "--embeddings", "--provider", "fake", "--json"])["embeddings"]
            ["dimensions"],
        64
    );
}

#[test]
fn provider_resolution_failure_preserves_vectors() {
    let repo = Repo::new();
    repo.json(&["embed", "--all", "--json"]);
    let before = repo.vectors();
    repo.configure("unknown-provider", 32);
    assert!(!repo
        .cli(&["reindex", "--embeddings", "--json"])
        .status
        .success());
    assert_eq!(before, repo.vectors());
}

#[test]
fn per_representation_failure_is_reported_with_nonzero_exit() {
    let repo = Repo::new();
    // A broken persisted representation must not be reported as successful
    // indexing merely because the engine continues processing other depths.
    repo.store
        .connection()
        .execute(
            "UPDATE memory_representations SET content = '' WHERE representation_type = 'summary'",
            [],
        )
        .unwrap();
    let out = repo.cli(&["embed", "--all", "--json"]);
    assert!(!out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["failed"].as_array().unwrap().len(), 1);
    assert_eq!(report["embedded"].as_array().unwrap().len(), 1);
}
