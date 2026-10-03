//! End-to-end review listing, explicit batch deletion, and project scope.

use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryId, MemoryStatus, MemoryType, NewMemory, ProjectId};
use vestige_store::Store;

struct Repo {
    tmp: TempDir,
    store: Store,
    project: ProjectId,
}

impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".vestige")).unwrap();
        let database = tmp.path().join("memory.sqlite");
        let config = format!(
            "project_id = \"proj_review-smoke\"\nproject_name = \"Review\"\nscope = \"project\"\n[storage]\nmode = \"user_data\"\npath = {:?}\n",
            database.to_str().unwrap()
        );
        std::fs::write(tmp.path().join(".vestige/config.toml"), config).unwrap();
        let mut store = Store::open(database).unwrap();
        let project = ProjectId::from_slug("review-smoke");
        store
            .ensure_project(&project, "Review", None, None)
            .unwrap();
        Self {
            tmp,
            store,
            project,
        }
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vestige"))
            .current_dir(self.tmp.path())
            .args(args)
            .output()
            .unwrap()
    }

    fn seed(&mut self, memory_type: MemoryType, importance: f64, age_days: u32) -> MemoryId {
        let bundle = build_bundle(
            &self.project,
            NewMemory {
                r#type: memory_type,
                body: "A fixture worth checking during review.",
                importance,
                source: None,
            },
        )
        .unwrap();
        let id = bundle.memory.id.clone();
        self.store.record_memory(&bundle).unwrap();
        self.store.connection().execute(
            "UPDATE memories SET created_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?2) WHERE id = ?1",
            rusqlite::params![id.as_str(), format!("-{age_days} days")],
        ).unwrap();
        id
    }

    fn snapshot(&self) -> Vec<Vec<Vec<String>>> {
        [
            "memories",
            "memory_representations",
            "memory_events",
            "query_events",
        ]
        .iter()
        .map(|table| {
            let mut stmt = self
                .store
                .connection()
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = stmt.column_count();
            stmt.query_map([], |row| {
                (0..columns)
                    .map(|column| Ok(format!("{:?}", row.get_ref(column)?)))
                    .collect()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<Vec<String>>>>()
            .unwrap()
        })
        .collect()
    }
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn review_lists_without_writes_then_batch_forgets_with_partial_failure() {
    let mut repo = Repo::new();
    let newer = repo.seed(MemoryType::Note, 0.3, 40);
    let older = repo.seed(MemoryType::Observation, 0.1, 100);
    let decision = repo.seed(MemoryType::Decision, 0.1, 100);
    repo.seed(MemoryType::Note, 0.3, 5);
    repo.seed(MemoryType::Note, 0.5, 100);
    let before = repo.snapshot();
    let first = repo.cli(&["review", "--json"]);
    let second = repo.cli(&["review", "--json"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let cards = json(&first);
    let cards = cards.as_array().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0]["id"], older.as_str());
    assert_eq!(cards[1]["id"], newer.as_str());
    for card in cards {
        assert_eq!(card.as_object().unwrap().len(), 7);
        assert!(card["age_days"].as_i64().unwrap() >= 40);
        assert_eq!(card["recall_count"], 0);
        assert_eq!(card["expand_count"], 0);
        assert!(card["one_liner"].as_str().unwrap().contains("fixture"));
        assert!(card["type"].is_string());
        assert!(card["importance"].is_number());
    }
    assert_eq!(before, repo.snapshot());

    let unknown = MemoryId::new();
    let batch = format!("{older},{unknown},invalid,{newer},{older}");
    let output = repo.cli(&["review", "--forget", &batch, "--json"]);
    assert!(!output.status.success());
    let outcomes = json(&output);
    assert_eq!(outcomes[0]["status"], "deleted");
    assert_eq!(outcomes[1]["status"], "not_found");
    assert_eq!(outcomes[2]["status"], "error");
    assert_eq!(outcomes[3]["status"], "deleted");
    assert_eq!(outcomes[4]["status"], "already_deleted");
    for id in [&older, &newer] {
        let fetched = repo.store.get_memory(id).unwrap().unwrap();
        assert_eq!(fetched.memory.status, MemoryStatus::Deleted);
        assert!(fetched.memory.deleted_at.is_some());
    }
    assert_eq!(
        repo.store
            .get_memory(&decision)
            .unwrap()
            .unwrap()
            .memory
            .status,
        MemoryStatus::Active
    );
    assert_eq!(
        json(&repo.cli(&["review", "--json"])),
        serde_json::json!([])
    );
    let restored = repo.cli(&["restore", older.as_str(), "--json"]);
    assert!(restored.status.success());
    assert_eq!(
        json(&repo.cli(&["review", "--json"]))[0]["id"],
        older.as_str()
    );
}

#[test]
fn review_forget_cannot_cross_project_scope() {
    let mut repo = Repo::new();
    let other = ProjectId::from_slug("other");
    repo.store
        .ensure_project(&other, "Other", None, None)
        .unwrap();
    let bundle = build_bundle(
        &other,
        NewMemory {
            r#type: MemoryType::Note,
            body: "Other project's note.",
            importance: 0.1,
            source: None,
        },
    )
    .unwrap();
    repo.store.record_memory(&bundle).unwrap();
    let output = repo.cli(&["review", "--forget", bundle.memory.id.as_str(), "--json"]);
    assert!(!output.status.success());
    assert_eq!(json(&output)[0]["status"], "out_of_scope");
    assert_eq!(
        repo.store
            .get_memory(&bundle.memory.id)
            .unwrap()
            .unwrap()
            .memory
            .status,
        MemoryStatus::Active
    );
}

#[test]
fn review_rejects_invalid_thresholds_before_mutating() {
    let repo = Repo::new();
    for value in ["NaN", "inf", "1.1"] {
        assert!(!repo
            .cli(&["review", "--importance-ceiling", value, "--json"])
            .status
            .success());
    }
}
