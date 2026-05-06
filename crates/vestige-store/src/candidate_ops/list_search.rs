//! Bulk reads — `list_candidates` and FTS5 `search_candidates_lexical`.

use std::str::FromStr;

use rusqlite::OptionalExtension;
use vestige_core::{Candidate, CandidateId, CandidateStatus, MemoryType, ProjectId};

use crate::helpers::invalid_id_to_sqlite;
use crate::{Result, Store};

use super::row_to_candidate;

// === PUBLIC TYPES ===

/// Filter parameters for listing candidates.
///
/// Defaults: `status = Some(Pending)`, `limit = Some(50)`, `include_rejected = false`.
/// When `include_rejected = true`, the `status` field is ignored and both
/// `pending` and `rejected` rows are returned.
#[derive(Debug, Clone, Default)]
pub struct CandidateFilter {
    /// Filter by lifecycle status. Defaults to `Pending` when `None` and
    /// `include_rejected` is `false`.
    pub status: Option<CandidateStatus>,
    /// Filter by proposed memory type.
    pub proposed_type: Option<MemoryType>,
    /// Maximum rows returned. Defaults to 50.
    pub limit: Option<u32>,
    /// When `true`, return both `pending` and `rejected` rows; `status` is ignored.
    pub include_rejected: bool,
}

/// A lightweight candidate match returned from FTS dedup search.
#[derive(Debug, Clone)]
pub struct CandidateHit {
    /// Candidate identifier.
    pub id: CandidateId,
    /// Proposed memory type of the matching candidate.
    pub proposed_type: MemoryType,
    /// BM25 score — lower is a better match (FTS5 convention).
    pub score: f32,
    /// Short snippet from the matching content.
    pub snippet: String,
}

// === STORE METHODS ===

impl Store {
    /// List candidates for a project, filtered by status and type.
    ///
    /// Default behaviour returns only `pending` candidates. Set
    /// `filter.include_rejected = true` to include `rejected` ones too.
    /// Results are ordered by `created_at DESC`.
    ///
    /// Sources are **not** loaded — call `get_candidate` for full detail.
    pub fn list_candidates(
        &self,
        project_id: &ProjectId,
        filter: &CandidateFilter,
    ) -> Result<Vec<Candidate>> {
        let mut sql = String::from(
            "SELECT id, project_id, proposed_type, status, title, one_liner, summary,
                    full_body, rationale, confidence, importance,
                    duplicate_of_memory_id, duplicate_of_candidate_id,
                    approved_memory_id, rejection_reason, review_note,
                    created_at, updated_at, reviewed_at
             FROM candidate_memories
             WHERE project_id = ?1",
        );

        if filter.include_rejected {
            sql.push_str(" AND status IN ('pending', 'rejected')");
        } else {
            let status_str = filter
                .status
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("pending");
            sql.push_str(&format!(" AND status = '{status_str}'"));
        }

        if filter.proposed_type.is_some() {
            sql.push_str(" AND proposed_type = ?2");
        }

        sql.push_str(" ORDER BY datetime(created_at) DESC");

        let limit = filter.limit.unwrap_or(50);
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = self.connection().prepare(&sql)?;

        let candidates: Vec<Candidate> = match &filter.proposed_type {
            Some(t) => stmt
                .query_map(
                    rusqlite::params![project_id.as_str(), t.as_str()],
                    row_to_candidate,
                )?
                .collect::<std::result::Result<_, _>>()?,
            None => stmt
                .query_map(rusqlite::params![project_id.as_str()], row_to_candidate)?
                .collect::<std::result::Result<_, _>>()?,
        };

        Ok(candidates)
    }

    /// FTS5-backed dedup search over pending candidates in the project.
    ///
    /// Queries `candidate_fts` (pending rows only, enforced by trigger in
    /// migration 0004) and also applies an explicit `status = 'pending'` filter
    /// as belt-and-braces. Returns compact [`CandidateHit`]s ordered by bm25
    /// score (best first, i.e. lowest raw bm25 value).
    ///
    /// Sources are not loaded — this is a dedup probe path, not a display path.
    pub fn search_candidates_lexical(
        &self,
        project_id: &ProjectId,
        fts_query: &str,
        filter: &CandidateFilter,
    ) -> Result<Vec<CandidateHit>> {
        if fts_query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let limit = filter.limit.unwrap_or(50);
        let candidate_limit = (limit as i64).saturating_mul(4).max(50);

        let mut fts_stmt = self.connection().prepare(
            "SELECT candidate_id, proposed_type, bm25(candidate_fts) AS score,
                    snippet(candidate_fts, 2, '', '', '…', 8)
             FROM candidate_fts
             WHERE candidate_fts MATCH ?1
             ORDER BY score ASC
             LIMIT ?2",
        )?;

        let raw: Vec<(String, String, f64, String)> = fts_stmt
            .query_map(rusqlite::params![fts_query, candidate_limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;

        let mut hits = Vec::new();
        for (id_str, type_str, score, snippet) in raw {
            let id = CandidateId::from_str(&id_str).map_err(invalid_id_to_sqlite)?;
            let proposed_type = MemoryType::from_str(&type_str).map_err(invalid_id_to_sqlite)?;

            // Belt-and-braces: verify status = pending and project scope.
            let status_row: Option<(String, String)> = self
                .connection()
                .query_row(
                    "SELECT status, project_id FROM candidate_memories WHERE id = ?1",
                    rusqlite::params![id_str],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            let Some((status_str, proj_str)) = status_row else {
                continue;
            };
            if proj_str != project_id.as_str() || status_str != "pending" {
                continue;
            }

            // Apply type filter if set.
            if let Some(t) = &filter.proposed_type {
                if proposed_type != *t {
                    continue;
                }
            }

            hits.push(CandidateHit {
                id,
                proposed_type,
                score: score as f32,
                snippet,
            });

            if hits.len() as u32 >= limit {
                break;
            }
        }

        Ok(hits)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vestige_core::{
        build_candidate_bundle, MemoryId, MemoryType, NewCandidate, ProjectId, RejectionReason,
    };

    use crate::Store;

    use super::CandidateFilter;

    fn open_store(tmp: &TempDir) -> Store {
        Store::open(tmp.path().join("memory.sqlite")).unwrap()
    }

    fn insert_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test", Some("/tmp/test"), None)
            .unwrap();
    }

    fn record_decision(
        store: &mut Store,
        proj: &ProjectId,
        body: &str,
    ) -> vestige_core::CandidateId {
        let bundle = build_candidate_bundle(NewCandidate {
            project_id: proj.clone(),
            proposed_type: MemoryType::Decision,
            body: body.to_string(),
            rationale: None,
            title_override: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        })
        .unwrap();
        let id = bundle.id.clone();
        store.record_candidate(&bundle).unwrap();
        id
    }

    #[test]
    fn list_candidates_default_returns_pending_only() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-list");
        insert_project(&mut store, &proj);

        let id1 = record_decision(&mut store, &proj, "Decision A: use Rust.");
        let id2 = record_decision(&mut store, &proj, "Decision B: use SQLite.");

        // Reject one
        store
            .mark_candidate_rejected(&id2, &RejectionReason::TooNoisy, None, None)
            .unwrap();

        let filter = CandidateFilter::default();
        let results = store.list_candidates(&proj, &filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id1);
    }

    #[test]
    fn list_candidates_include_rejected_returns_both() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-include-rejected");
        insert_project(&mut store, &proj);

        let _id1 = record_decision(&mut store, &proj, "Decision A.");
        let id2 = record_decision(&mut store, &proj, "Decision B.");
        store
            .mark_candidate_rejected(&id2, &RejectionReason::Stale, None, None)
            .unwrap();

        let filter = CandidateFilter {
            include_rejected: true,
            ..Default::default()
        };
        let results = store.list_candidates(&proj, &filter).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn list_candidates_project_scope_isolation() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);

        let proj_a = ProjectId::from_slug("project-a");
        let proj_b = ProjectId::from_slug("project-b");
        insert_project(&mut store, &proj_a);
        insert_project(&mut store, &proj_b);

        record_decision(&mut store, &proj_a, "Decision in project A.");
        record_decision(&mut store, &proj_a, "Another A decision.");

        let filter = CandidateFilter::default();
        let results_b = store.list_candidates(&proj_b, &filter).unwrap();
        assert!(
            results_b.is_empty(),
            "project B should see no candidates from project A"
        );

        let results_a = store.list_candidates(&proj_a, &filter).unwrap();
        assert_eq!(results_a.len(), 2);
    }

    #[test]
    fn search_candidates_lexical_returns_only_pending() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-search");
        insert_project(&mut store, &proj);

        let id = record_decision(
            &mut store,
            &proj,
            "SQLite FTS5 enables fast full-text search.",
        );

        // Approve it — should vanish from FTS.
        let mem_id = MemoryId::new();
        store.mark_candidate_approved(&id, &mem_id).unwrap();

        let filter = CandidateFilter::default();
        let hits = store
            .search_candidates_lexical(&proj, "SQLite", &filter)
            .unwrap();
        assert!(
            hits.is_empty(),
            "approved candidate must not appear in FTS search"
        );
    }

    #[test]
    fn search_candidates_lexical_finds_pending_match() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-search-hit");
        insert_project(&mut store, &proj);

        record_decision(
            &mut store,
            &proj,
            "Rust ownership model ensures memory safety without GC.",
        );

        let filter = CandidateFilter::default();
        let hits = store
            .search_candidates_lexical(&proj, "memory safety", &filter)
            .unwrap();
        assert!(!hits.is_empty(), "should find the pending candidate");
        assert_eq!(hits[0].proposed_type, MemoryType::Decision);
    }

    #[test]
    fn search_candidates_lexical_empty_query_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-empty-q");
        insert_project(&mut store, &proj);

        record_decision(&mut store, &proj, "Some content here.");
        let filter = CandidateFilter::default();
        let hits = store
            .search_candidates_lexical(&proj, "   ", &filter)
            .unwrap();
        assert!(hits.is_empty());
    }
}
