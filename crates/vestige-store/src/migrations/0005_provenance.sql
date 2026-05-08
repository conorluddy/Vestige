-- Migration 0005: provenance and receipts — query_events, memory_events.memory_id, memory_provenance view (PRD §8)
--
-- V0.3 introduces the Provenance and Receipts layer:
--   - query_events: append-only trace table for every recall call (search/expand/context).
--     Separate from memory_events — far higher cardinality, different retention pressure.
--   - memory_events.memory_id: indexed nullable column for direct journal lookups without
--     JSON extraction. Backfilled from payload_json for existing rows.
--   - memory_provenance: convenience view pre-joining memories and memory_events via the
--     new indexed column. Used by `vestige why` without re-deriving the join.
--
-- Column notes:
--   - All timestamps are RFC-3339 TEXT in UTC. SQLite has no native timestamp type.
--   - query_text is capped at 1 KiB by application code (UTF-8 boundary); column is TEXT, unconstrained.
--   - result_ids_json / result_scores_json are JSON arrays; null for non-search kinds.
--   - memory_events.memory_id is nullable: some events are project-scoped without a memory.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Query events table (PRD §8.2)
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE query_events (
    id                  TEXT PRIMARY KEY,           -- trace_<ULID>
    project_id          TEXT NOT NULL,
    kind                TEXT NOT NULL,              -- search | expand | context
    mode_requested      TEXT,                       -- lexical | semantic | hybrid (search only; null otherwise)
    mode_resolved       TEXT,                       -- actual mode after fallback (null for non-search)
    query_text          TEXT,                       -- ≤ 1 KiB, truncated at UTF-8 boundary by caller
    params_json         TEXT,                       -- limit, type filter, depth, replay_of, etc.
    caller              TEXT NOT NULL,              -- cli | mcp
    provider            TEXT,                       -- e.g. "fastembed"; null for lexical / non-search
    provider_model      TEXT,                       -- e.g. "BAAI/bge-small-en-v1.5"; null when provider is null
    result_ids_json     TEXT,                       -- ordered JSON array of mem_<ULID>; null for context/expand
    result_scores_json  TEXT,                       -- parallel score array; null for non-search kinds
    result_count        INTEGER NOT NULL DEFAULT 0,
    latency_ms          INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE INDEX idx_query_events_project_created
    ON query_events (project_id, created_at DESC);

CREATE INDEX idx_query_events_kind
    ON query_events (project_id, kind, created_at DESC);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Indexed memory_id column on memory_events (PRD §8.3)
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE memory_events ADD COLUMN memory_id TEXT;

CREATE INDEX idx_memory_events_memory_id
    ON memory_events (memory_id, created_at DESC);

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Backfill memory_id from payload_json for existing rows (PRD §8.3)
--
-- Handles the four payload variants that carry a memory_id key:
--   memory.recorded    → payload { "memory_id": "mem_..." }
--   memory.forgotten   → payload { "memory_id": "mem_..." }
--   memory.restored    → payload { "memory_id": "mem_..." }
--   candidate.approved → payload { "candidate_id": "cand_...", "memory_id": "mem_..." }
--
-- json_extract returns NULL when the key is absent, so the WHERE clause is
-- satisfied only for rows that actually carry the key. Unknown event types
-- whose payloads happen to contain a "memory_id" key are also backfilled —
-- that is conservative but harmless.
-- ─────────────────────────────────────────────────────────────────────────────

UPDATE memory_events
SET memory_id = json_extract(payload_json, '$.memory_id')
WHERE json_extract(payload_json, '$.memory_id') IS NOT NULL
  AND memory_id IS NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Memory provenance view (PRD §8.5)
--
-- LEFT JOIN so memories with no events still appear (e.g. a freshly recorded
-- memory before any status transition). Ordered by event timestamp for walk
-- rendering. Uses the new indexed memory_id column — no JSON extraction.
-- ─────────────────────────────────────────────────────────────────────────────

CREATE VIEW memory_provenance AS
SELECT
    m.id              AS memory_id,
    m.project_id      AS project_id,
    e.id              AS event_id,
    e.event_type      AS event_type,
    e.payload_json    AS payload_json,
    e.created_at      AS event_at
FROM memories m
LEFT JOIN memory_events e
       ON e.memory_id = m.id
ORDER BY e.created_at;
