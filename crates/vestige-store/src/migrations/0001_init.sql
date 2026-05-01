-- Migration 0001: core schema — projects, memories, representations, sources, events (PRD §9)
--
-- Establishes the three source-of-truth layers:
--   1. memory_events — append-only audit journal
--   2. memories + memory_representations + memory_sources — derived interpretation
-- The FTS acceleration layer is added in migration 0002.
--
-- All timestamps are RFC-3339 TEXT in UTC. SQLite has no native timestamp type.
-- Status column values: 'active' | 'deleted' (soft-delete only — no DELETE FROM memories).

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    root_path   TEXT,
    git_remote  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE memories (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL,
    type        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    confidence  REAL NOT NULL DEFAULT 1.0,
    importance  REAL NOT NULL DEFAULT 0.5,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE INDEX idx_memories_project_status ON memories (project_id, status);
CREATE INDEX idx_memories_type ON memories (type);

CREATE TABLE memory_representations (
    id                  TEXT PRIMARY KEY,
    memory_id           TEXT NOT NULL,
    representation_type TEXT NOT NULL,
    content             TEXT NOT NULL,
    token_count         INTEGER,
    content_hash        TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE,
    UNIQUE (memory_id, representation_type)
);

CREATE INDEX idx_repr_memory ON memory_representations (memory_id);

CREATE TABLE memory_sources (
    id              TEXT PRIMARY KEY,
    memory_id       TEXT NOT NULL,
    source_type     TEXT NOT NULL,
    source_ref      TEXT,
    source_content  TEXT,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);

CREATE INDEX idx_sources_memory ON memory_sources (memory_id);

CREATE TABLE memory_events (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    event_type   TEXT NOT NULL,
    payload_json TEXT,
    created_at   TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id)
);
