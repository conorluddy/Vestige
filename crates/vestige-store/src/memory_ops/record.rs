//! Memory write path — `record_memory`, `record_event`, `append_status_event`.
//!
//! Every mutation in this file appends a row to `memory_events` so the durable
//! journal stays the canonical audit trail (PRD §11.5).

use time::OffsetDateTime;
use ulid::Ulid;

use vestige_core::{MemoryBundle, MemoryId, ProjectId};

use crate::helpers::rfc3339;
use crate::{Result, Store};

impl Store {
    /// Append a structured event to the `memory_events` journal.
    ///
    /// The journal is append-only — this is the only write path. `event_type`
    /// should follow dot-namespaced convention (`"memory.recorded"`,
    /// `"memory.forgotten"`, etc.). `payload_json` is optional free-form JSON.
    ///
    /// Side-effect: inserts one row into `memory_events`.
    pub fn record_event(
        &self,
        project_id: &ProjectId,
        event_type: &str,
        payload_json: Option<&str>,
    ) -> Result<()> {
        let id = format!("evt_{}", Ulid::new());
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        self.connection().execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, project_id.as_str(), event_type, payload_json, now_str],
        )?;
        Ok(())
    }

    /// Persist a [`MemoryBundle`] and record a `memory.recorded` journal event.
    ///
    /// **Atomicity** — everything runs inside a single `BEGIN … COMMIT`
    /// transaction: the `memories` row, all `memory_representations` rows
    /// (typically four: handle / one-liner / summary / compressed), the
    /// optional `memory_sources` row, and the `memory_events` entry. Either
    /// all rows land or none do.
    ///
    /// **FTS** — `memory_representations` INSERT triggers (migration 0002)
    /// automatically populate `memory_fts` within the same transaction.
    ///
    /// Side-effects: inserts into `memories`, `memory_representations`,
    /// optionally `memory_sources`, and `memory_events`.
    pub fn record_memory(&mut self, bundle: &MemoryBundle) -> Result<()> {
        let tx = self.connection_mut().transaction()?;
        let m = &bundle.memory;
        let created_str = rfc3339(m.created_at)?;
        let updated_str = rfc3339(m.updated_at)?;

        tx.execute(
            "INSERT INTO memories (id, project_id, type, status, confidence, importance, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                m.id.as_str(),
                m.project_id.as_str(),
                m.r#type.as_str(),
                m.status.as_str(),
                m.confidence,
                m.importance,
                created_str,
                updated_str,
            ],
        )?;

        for rep in &bundle.representations {
            let id = format!("rep_{}", Ulid::new());
            tx.execute(
                "INSERT INTO memory_representations
                    (id, memory_id, representation_type, content, token_count, content_hash, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)",
                rusqlite::params![
                    id,
                    rep.memory_id.as_str(),
                    rep.depth.as_str(),
                    rep.content,
                    rep.content_hash,
                    created_str,
                ],
            )?;
        }

        if let Some(src) = &bundle.source {
            let id = format!("src_{}", Ulid::new());
            tx.execute(
                "INSERT INTO memory_sources
                    (id, memory_id, source_type, source_ref, source_content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    src.memory_id.as_str(),
                    src.source_type,
                    src.source_ref,
                    src.source_content,
                    created_str,
                ],
            )?;
        }

        let payload = serde_json::json!({
            "memory_id": m.id.as_str(),
            "type": m.r#type.as_str(),
            "importance": m.importance,
            "has_source": bundle.source.is_some(),
            "source_truncated": bundle.source.as_ref().map(|s| s.truncated).unwrap_or(false),
        })
        .to_string();
        let event_id = format!("evt_{}", Ulid::new());
        tx.execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                event_id,
                m.project_id.as_str(),
                "memory.recorded",
                payload,
                created_str,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Insert a single source row into `memory_sources` for an existing memory.
    ///
    /// Used by the approval path in `vestige-engine` to attach candidate provenance
    /// and reverse-provenance links after `record_memory` has already committed the
    /// memory bundle. The store has no multi-source variant of `record_memory`
    /// (single-source is the common case); callers that need extra rows call this.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] on any SQLite failure (FK violation if the
    /// `memory_id` does not exist, I/O errors, etc.).
    pub fn add_memory_source(
        &mut self,
        memory_id: &MemoryId,
        source_type: &str,
        source_ref: Option<&str>,
        source_content: Option<&str>,
    ) -> Result<()> {
        use ulid::Ulid;
        let id = format!("src_{}", Ulid::new());
        let now_str = rfc3339(OffsetDateTime::now_utc())?;
        self.connection().execute(
            "INSERT INTO memory_sources
                 (id, memory_id, source_type, source_ref, source_content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                id,
                memory_id.as_str(),
                source_type,
                source_ref,
                source_content,
                now_str
            ],
        )?;
        Ok(())
    }

    /// Append a status-transition event for `id` to the `memory_events` journal.
    ///
    /// Looks up `project_id` from the `memories` row, then inserts one
    /// `memory_events` row with `{ "memory_id": "…" }` as the payload.
    /// Called by `forget_memory` and `restore_memory`; not for direct use.
    pub(crate) fn append_status_event(
        &self,
        id: &MemoryId,
        event_type: &str,
        when: &str,
    ) -> Result<()> {
        let project_id: String = self.connection().query_row(
            "SELECT project_id FROM memories WHERE id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )?;
        let payload = serde_json::json!({ "memory_id": id.as_str() }).to_string();
        let event_id = format!("evt_{}", Ulid::new());
        self.connection().execute(
            "INSERT INTO memory_events (id, project_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![event_id, project_id, event_type, payload, when],
        )?;
        Ok(())
    }
}
