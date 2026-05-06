//! Candidate lifecycle transitions — `mark_candidate_approved`, `mark_candidate_rejected`.
//!
//! Both methods validate `status = 'pending'` before mutating. Either one failing
//! returns a `StoreError::Corruption` mapping to the `CandidateNotPending` semantic.
//! The `candidate_fts_after_update` trigger (migration 0004) removes the FTS row
//! automatically when status leaves `'pending'` — no explicit cleanup here.

use rusqlite::OptionalExtension;
use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::{CandidateId, MemoryId, RejectionReason};

use crate::helpers::rfc3339;
use crate::{Result, Store, StoreError};

impl Store {
    /// Mark a pending candidate as approved and link it to the promoted memory.
    ///
    /// Validates that the candidate exists and has `status = 'pending'` — returns
    /// `StoreError::Corruption` with a `CandidateNotPending` message otherwise.
    /// Sets `status='approved'`, `approved_memory_id`, `reviewed_at`, `updated_at`
    /// in a single transaction and appends a `candidate.approved` journal event.
    pub fn mark_candidate_approved(
        &mut self,
        id: &CandidateId,
        memory_id: &MemoryId,
    ) -> Result<()> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;

        let tx = self.connection_mut().transaction()?;

        // Validate current status.
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM candidate_memories WHERE id = ?1",
                rusqlite::params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?;

        match status.as_deref() {
            None => {
                return Err(StoreError::Corruption(format!(
                    "CandidateNotFound: no candidate row for `{}`",
                    id.as_str()
                )))
            }
            Some("pending") => {}
            Some(s) => {
                return Err(StoreError::Corruption(format!(
                    "CandidateNotPending: candidate `{}` has status `{s}`, expected `pending`",
                    id.as_str()
                )))
            }
        }

        tx.execute(
            "UPDATE candidate_memories
             SET status = 'approved',
                 approved_memory_id = ?2,
                 reviewed_at = ?3,
                 updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![id.as_str(), memory_id.as_str(), now_str],
        )?;

        let payload = serde_json::json!({
            "candidate_id": id.as_str(),
            "memory_id": memory_id.as_str(),
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        let project_id: String = tx.query_row(
            "SELECT project_id FROM candidate_memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![event_id, project_id, "candidate.approved", payload, now_str,],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Mark a pending candidate as rejected with a reason and optional provenance.
    ///
    /// Validates `status = 'pending'` before mutating. Sets `status='rejected'`,
    /// `rejection_reason`, `duplicate_of_memory_id` (when provided), `review_note`,
    /// `reviewed_at`, and `updated_at` in one transaction. Appends a
    /// `candidate.rejected` event to the journal.
    pub fn mark_candidate_rejected(
        &mut self,
        id: &CandidateId,
        reason: &RejectionReason,
        duplicate_of: Option<&MemoryId>,
        review_note: Option<&str>,
    ) -> Result<()> {
        let now_str = rfc3339(OffsetDateTime::now_utc())?;

        let tx = self.connection_mut().transaction()?;

        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM candidate_memories WHERE id = ?1",
                rusqlite::params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?;

        match status.as_deref() {
            None => {
                return Err(StoreError::Corruption(format!(
                    "CandidateNotFound: no candidate row for `{}`",
                    id.as_str()
                )))
            }
            Some("pending") => {}
            Some(s) => {
                return Err(StoreError::Corruption(format!(
                    "CandidateNotPending: candidate `{}` has status `{s}`, expected `pending`",
                    id.as_str()
                )))
            }
        }

        tx.execute(
            "UPDATE candidate_memories
             SET status = 'rejected',
                 rejection_reason = ?2,
                 duplicate_of_memory_id = ?3,
                 review_note = ?4,
                 reviewed_at = ?5,
                 updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![
                id.as_str(),
                reason.as_str().as_ref(),
                duplicate_of.map(|m| m.as_str()),
                review_note,
                now_str,
            ],
        )?;

        let payload = serde_json::json!({
            "candidate_id": id.as_str(),
            "reason": reason.as_str().as_ref() as &str,
            "duplicate_of_memory_id": duplicate_of.map(|m| m.as_str()),
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        let project_id: String = tx.query_row(
            "SELECT project_id FROM candidate_memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![event_id, project_id, "candidate.rejected", payload, now_str,],
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
        build_candidate_bundle, CandidateStatus, MemoryId, MemoryType, NewCandidate, ProjectId,
        RejectionReason,
    };

    use crate::{Store, StoreError};

    fn open_store(tmp: &TempDir) -> Store {
        Store::open(tmp.path().join("memory.sqlite")).unwrap()
    }

    fn insert_project(store: &mut Store, project_id: &ProjectId) {
        store
            .ensure_project(project_id, "Test", Some("/tmp/test"), None)
            .unwrap();
    }

    fn record_candidate(store: &mut Store, proj: &ProjectId) -> vestige_core::CandidateId {
        let bundle = build_candidate_bundle(NewCandidate {
            project_id: proj.clone(),
            proposed_type: MemoryType::Observation,
            body: "Cargo workspaces improve build isolation.".to_string(),
            rationale: None,
            title_override: None,
            importance: 0.5,
            confidence: 0.9,
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
    fn approve_flips_status_to_approved() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-approve");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        let mem_id = MemoryId::new();
        store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

        let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
        assert_eq!(candidate.status, CandidateStatus::Approved);
        assert_eq!(candidate.approved_memory_id.as_ref(), Some(&mem_id));
        assert!(candidate.reviewed_at.is_some());
    }

    #[test]
    fn approve_twice_fails_with_not_pending_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-approve-twice");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        let mem_id = MemoryId::new();
        store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

        let second_mem = MemoryId::new();
        let err = store
            .mark_candidate_approved(&cand_id, &second_mem)
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Corruption(ref msg) if msg.contains("CandidateNotPending")),
            "expected CandidateNotPending, got: {err}"
        );
    }

    #[test]
    fn reject_persists_reason_and_duplicate_link() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-reject");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        let dup_mem = MemoryId::new();
        store
            .mark_candidate_rejected(
                &cand_id,
                &RejectionReason::Duplicate,
                Some(&dup_mem),
                Some("Already in memory store."),
            )
            .unwrap();

        let candidate = store.get_candidate(&cand_id).unwrap().unwrap();
        assert_eq!(candidate.status, CandidateStatus::Rejected);
        assert_eq!(candidate.rejection_reason, Some(RejectionReason::Duplicate));
        assert_eq!(candidate.duplicate_of_memory_id.as_ref(), Some(&dup_mem));
        assert_eq!(
            candidate.review_note.as_deref(),
            Some("Already in memory store.")
        );
    }

    #[test]
    fn reject_after_approve_fails_with_not_pending_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-reject-after-approve");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        let mem_id = MemoryId::new();
        store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

        let err = store
            .mark_candidate_rejected(&cand_id, &RejectionReason::Wrong, None, None)
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Corruption(ref msg) if msg.contains("CandidateNotPending")),
            "expected CandidateNotPending, got: {err}"
        );
    }

    #[test]
    fn approve_unknown_id_fails_with_not_found_error() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);

        let unknown = vestige_core::CandidateId::generate();
        let mem_id = MemoryId::new();
        let err = store
            .mark_candidate_approved(&unknown, &mem_id)
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Corruption(ref msg) if msg.contains("CandidateNotFound")),
            "expected CandidateNotFound, got: {err}"
        );
    }

    #[test]
    fn approve_emits_approved_event() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-approve-event");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        let mem_id = MemoryId::new();
        store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

        let count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM memory_events WHERE event_type = 'candidate.approved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn reject_emits_rejected_event() {
        let tmp = TempDir::new().unwrap();
        let mut store = open_store(&tmp);
        let proj = ProjectId::from_slug("test-reject-event");
        insert_project(&mut store, &proj);

        let cand_id = record_candidate(&mut store, &proj);
        store
            .mark_candidate_rejected(&cand_id, &RejectionReason::TooNoisy, None, None)
            .unwrap();

        let count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM memory_events WHERE event_type = 'candidate.rejected'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
