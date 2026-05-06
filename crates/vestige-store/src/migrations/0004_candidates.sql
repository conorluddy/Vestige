-- Migration 0004: candidate inbox — candidate_memories, candidate_sources, candidate_fts (PRD §8.3, §8.4)
--
-- Adds the assimilation inbox layer between raw agent capture and durable memory.
-- Candidates flow: pending → approved (promoted to mem_<ULID>) | rejected | superseded.
-- Pending and rejected candidates never enter normal recall (memory_fts is untouched).
--
-- All timestamps are RFC-3339 TEXT in UTC. SQLite has no native timestamp type.
-- Status column values: 'pending' | 'approved' | 'rejected' | 'superseded'.
-- The candidate_fts shadow table mirrors the memory_fts pattern from 0002_fts.sql —
-- triggered on status so approved/rejected candidates exit the dedup index automatically.

CREATE TABLE candidate_memories (
    id                          TEXT PRIMARY KEY,           -- cand_<ULID>
    project_id                  TEXT NOT NULL,
    proposed_type               TEXT NOT NULL,              -- decision|note|preference|open_question|observation|project_summary
    status                      TEXT NOT NULL DEFAULT 'pending',  -- pending|approved|rejected|superseded
    title                       TEXT NOT NULL,
    one_liner                   TEXT NOT NULL,
    summary                     TEXT,
    full_body                   TEXT NOT NULL,
    rationale                   TEXT,
    confidence                  REAL NOT NULL DEFAULT 0.5,
    importance                  REAL NOT NULL DEFAULT 0.5,
    duplicate_of_memory_id      TEXT,
    duplicate_of_candidate_id   TEXT,
    approved_memory_id          TEXT,
    rejection_reason            TEXT,
    review_note                 TEXT,
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL,
    reviewed_at                 TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE INDEX idx_candidate_memories_project_status
    ON candidate_memories(project_id, status, created_at DESC);

CREATE TABLE candidate_sources (
    id              TEXT PRIMARY KEY,
    candidate_id    TEXT NOT NULL,
    source_type     TEXT NOT NULL,
    source_ref      TEXT,
    source_content  TEXT,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (candidate_id) REFERENCES candidate_memories(id) ON DELETE CASCADE
);

-- FTS5 dedup index over pending candidates only. Content: title + full_body.
-- Mirrors memory_fts from 0002_fts.sql; triggers keep it in sync with status.
CREATE VIRTUAL TABLE candidate_fts USING fts5(
    candidate_id  UNINDEXED,
    proposed_type UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);

-- Triggers mirror the memory_fts pattern from 0002_fts.sql.

CREATE TRIGGER candidate_fts_after_insert AFTER INSERT ON candidate_memories
    WHEN NEW.status = 'pending'
BEGIN
    INSERT INTO candidate_fts(candidate_id, proposed_type, content)
    VALUES (NEW.id, NEW.proposed_type, NEW.title || ' ' || NEW.full_body);
END;

CREATE TRIGGER candidate_fts_after_update AFTER UPDATE OF status ON candidate_memories
BEGIN
    DELETE FROM candidate_fts WHERE candidate_id = NEW.id;
    -- Re-insert only if still pending.
    INSERT INTO candidate_fts(candidate_id, proposed_type, content)
    SELECT NEW.id, NEW.proposed_type, NEW.title || ' ' || NEW.full_body
    WHERE NEW.status = 'pending';
END;

CREATE TRIGGER candidate_fts_after_delete AFTER DELETE ON candidate_memories
BEGIN
    DELETE FROM candidate_fts WHERE candidate_id = OLD.id;
END;
