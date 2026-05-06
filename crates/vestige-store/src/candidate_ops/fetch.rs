//! Per-candidate reads — `get_candidate`, `fetch_candidate_sources`.

use vestige_core::{Candidate, CandidateId, CandidateSource};

use crate::{Result, Store};

use super::{row_to_candidate, row_to_candidate_source};

impl Store {
    /// Fetch a candidate by ID, joining all sources.
    ///
    /// Returns `None` if no row with that ID exists (any status). Callers
    /// that need to enforce project scope should verify `candidate.project_id`
    /// after retrieval, consistent with how `get_memory` works.
    pub fn get_candidate(&self, id: &CandidateId) -> Result<Option<Candidate>> {
        let mut stmt = self.connection().prepare(
            "SELECT id, project_id, proposed_type, status, title, one_liner, summary,
                    full_body, rationale, confidence, importance,
                    duplicate_of_memory_id, duplicate_of_candidate_id,
                    approved_memory_id, rejection_reason, review_note,
                    created_at, updated_at, reviewed_at
             FROM candidate_memories
             WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id.as_str()])?;
        let candidate = match rows.next()? {
            Some(row) => row_to_candidate(row)?,
            None => return Ok(None),
        };
        let sources = fetch_candidate_sources(self.connection(), id)?;
        Ok(Some(Candidate {
            sources,
            ..candidate
        }))
    }
}

/// Fetch all `candidate_sources` rows for `id`, ordered by `created_at`.
pub(crate) fn fetch_candidate_sources(
    conn: &rusqlite::Connection,
    candidate_id: &CandidateId,
) -> Result<Vec<CandidateSource>> {
    let mut stmt = conn.prepare(
        "SELECT source_type, source_ref, source_content
         FROM candidate_sources
         WHERE candidate_id = ?1
         ORDER BY created_at",
    )?;
    let sources = stmt
        .query_map(
            rusqlite::params![candidate_id.as_str()],
            row_to_candidate_source,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(sources)
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vestige_core::{build_candidate_bundle, MemoryType, NewCandidate, ProjectId};

    use crate::Store;

    fn open_store(tmp: &TempDir) -> Store {
        Store::open(tmp.path().join("memory.sqlite")).unwrap()
    }

    fn insert_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test", Some("/tmp/test"), None)
            .unwrap();
    }

    #[test]
    fn get_candidate_returns_none_for_unknown_id() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let id = vestige_core::CandidateId::generate();
        let result = store.get_candidate(&id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_candidate_round_trip_preserves_fields() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-fetch");
        insert_project(&mut store, &proj);

        let input = NewCandidate {
            project_id: proj.clone(),
            proposed_type: MemoryType::Preference,
            body: "Prefer functional style over imperative loops.".to_string(),
            rationale: Some("More composable.".to_string()),
            title_override: Some("Prefer functional style".to_string()),
            importance: 0.6,
            confidence: 0.85,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        };
        let bundle = build_candidate_bundle(input).unwrap();
        let cand_id = bundle.id.clone();
        store.record_candidate(&bundle).unwrap();

        let fetched = store.get_candidate(&cand_id).unwrap().unwrap();
        assert_eq!(fetched.id, cand_id);
        assert_eq!(fetched.project_id, proj);
        assert_eq!(fetched.proposed_type, MemoryType::Preference);
        assert_eq!(fetched.title, "Prefer functional style");
        assert_eq!(fetched.rationale.as_deref(), Some("More composable."));
        assert!((fetched.confidence - 0.85).abs() < 0.001);
        assert!((fetched.importance - 0.6).abs() < 0.001);
    }
}
