//! Candidate write path — `record_candidate`.
//!
//! Mirrors `memory_ops::record` in structure. Every candidate insert is fully
//! transactional: candidate row + sources + `candidate.proposed` audit event
//! land atomically or not at all.

use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::CandidateBundle;

use crate::helpers::rfc3339;
use crate::{Result, Store};

impl Store {
    /// Persist a [`CandidateBundle`] and record a `candidate.proposed` journal event.
    ///
    /// **Atomicity** — the `candidate_memories` row, all `candidate_sources` rows,
    /// and the `memory_events` entry all land in one transaction.
    ///
    /// **FTS** — the `candidate_fts_after_insert` trigger (migration 0004)
    /// automatically indexes the new candidate if `status = 'pending'`.
    ///
    /// Side-effects: inserts into `candidate_memories`, `candidate_sources`,
    /// and `memory_events`.
    pub fn record_candidate(&mut self, bundle: &CandidateBundle) -> Result<()> {
        let tx = self.connection_mut().transaction()?;
        let created_str = rfc3339(bundle.created_at)?;
        let now_str = rfc3339(OffsetDateTime::now_utc())?;

        tx.execute(
            "INSERT INTO candidate_memories (
                id, project_id, proposed_type, status, title, one_liner, summary,
                full_body, rationale, confidence, importance,
                duplicate_of_memory_id, duplicate_of_candidate_id,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                bundle.id.as_str(),
                bundle.project_id.as_str(),
                bundle.proposed_type.as_str(),
                bundle.title,
                bundle.one_liner,
                bundle.summary,
                bundle.full_body,
                bundle.rationale,
                bundle.confidence as f64,
                bundle.importance as f64,
                bundle.duplicate_of_memory_id.as_ref().map(|id| id.as_str()),
                bundle
                    .duplicate_of_candidate_id
                    .as_ref()
                    .map(|id| id.as_str()),
                created_str,
                now_str,
            ],
        )?;

        for src in &bundle.sources {
            let src_id = format!("csrc_{}", Ulid::new());
            tx.execute(
                "INSERT INTO candidate_sources (id, candidate_id, source_type, source_ref, source_content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    src_id,
                    bundle.id.as_str(),
                    src.source_type,
                    src.source_ref,
                    src.source_content,
                    created_str,
                ],
            )?;
        }

        let first_source_ref = bundle.sources.first().and_then(|s| s.source_ref.as_deref());
        let payload = serde_json::json!({
            "candidate_id": bundle.id.as_str(),
            "proposed_type": bundle.proposed_type.as_str(),
            "has_source": !bundle.sources.is_empty(),
            "source_ref": first_source_ref,
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                event_id,
                bundle.project_id.as_str(),
                "candidate.proposed",
                payload,
                created_str,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vestige_core::{
        build_candidate_bundle, CandidateStatus, MemoryType, NewCandidate, ProjectId,
    };

    use crate::Store;

    fn open_store(tmp: &TempDir) -> Store {
        Store::open(tmp.path().join("memory.sqlite")).unwrap()
    }

    fn insert_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test", Some("/tmp/test"), None)
            .unwrap();
    }

    fn make_bundle(project_id: &ProjectId) -> vestige_core::CandidateBundle {
        let input = NewCandidate {
            project_id: project_id.clone(),
            proposed_type: MemoryType::Decision,
            body: "Use SQLite for persistent storage because it is fast and portable.".to_string(),
            rationale: Some("No daemon, no network dependency.".to_string()),
            title_override: None,
            importance: 0.7,
            confidence: 0.9,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        };
        build_candidate_bundle(input).unwrap()
    }

    #[test]
    fn record_candidate_inserts_row() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-record");
        insert_project(&mut store, &proj);

        let bundle = make_bundle(&proj);
        let cand_id = bundle.id.clone();
        store.record_candidate(&bundle).unwrap();

        let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
        assert_eq!(candidate.id, cand_id);
        assert_eq!(candidate.status, CandidateStatus::Pending);
        assert_eq!(candidate.proposed_type, MemoryType::Decision);
        assert!(!candidate.title.is_empty());
        assert!(!candidate.full_body.is_empty());
    }

    #[test]
    fn record_candidate_emits_proposed_event() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-event");
        insert_project(&mut store, &proj);

        let bundle = make_bundle(&proj);
        store.record_candidate(&bundle).unwrap();

        let count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM memory_events WHERE event_type = 'candidate.proposed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn record_candidate_with_source_inserts_source_row() {
        use vestige_core::NewCandidateSource;

        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-source");
        insert_project(&mut store, &proj);

        let input = NewCandidate {
            project_id: proj.clone(),
            proposed_type: MemoryType::Note,
            body: "A note with a source file.".to_string(),
            rationale: None,
            title_override: None,
            importance: 0.5,
            confidence: 0.8,
            source: Some(NewCandidateSource {
                source_type: "file".to_string(),
                source_ref: Some("src/lib.rs".to_string()),
                source_content: Some("fn main() {}".to_string()),
            }),
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        };
        let bundle = build_candidate_bundle(input).unwrap();
        let cand_id = bundle.id.clone();
        store.record_candidate(&bundle).unwrap();

        let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
        assert_eq!(candidate.sources.len(), 1);
        assert_eq!(candidate.sources[0].source_type, "file");
        assert_eq!(
            candidate.sources[0].source_ref.as_deref(),
            Some("src/lib.rs")
        );
    }
}
