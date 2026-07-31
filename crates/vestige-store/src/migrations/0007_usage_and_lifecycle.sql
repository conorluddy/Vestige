-- Migration 0007: usage counters and supersede link (V0.6, issue #128)
--
-- Gives every memory a lightweight usage/lifecycle signal:
--   - recall_count / expand_count: how often a memory surfaces in search
--     results and how often it's expanded past the one-liner. Feeds ranking,
--     the context pack, and the future `vestige review` hygiene queue (which
--     memories never get used are candidates for pruning).
--   - last_recalled_at: RFC-3339 UTC TEXT, same convention as every other
--     timestamp column in this schema; NULL until the first recall.
--   - superseded_by: nullable mem_<ULID> of the memory that replaces this
--     one. Makes memory lineage walkable via `vestige why` without needing
--     a new event kind — the row itself carries the pointer.
--
-- Column notes:
--   - No FOREIGN KEY on superseded_by: the replacing memory may not exist
--     yet at the moment this column is set, and enforcing referential
--     integrity here would require a self-referential deferred constraint
--     SQLite handles awkwardly. Validity is a core-layer concern.
--   - The partial index only covers non-NULL rows, since superseded_by is
--     NULL for the overwhelming majority of memories.
--
-- ─────────────────────────────────────────────────────────────────────────────
-- IMPORTANT — trigger scope invariant (read before touching counter bumps)
--
-- Every trigger on `memories` is scoped `AFTER UPDATE OF status`
-- (0002_fts.sql:31,39 and 0003_embeddings.sql:74). A counter-only UPDATE
-- that never SETs `status` therefore cannot fire the FTS or
-- embedding-staleness triggers. Counter bumps must never widen into a
-- full-row UPDATE — an `UPDATE memories SET recall_count = ... WHERE id = ?`
-- that also happens to touch `status` (even to the same value, depending on
-- how the statement is built) risks spuriously re-firing soft-delete/restore
-- FTS sync or marking embeddings stale. Keep recall/expand bumps to their
-- own narrow `SET recall_count = ..., last_recalled_at = ...` statements.
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE memories ADD COLUMN recall_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN expand_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_recalled_at TEXT;
ALTER TABLE memories ADD COLUMN superseded_by TEXT;

CREATE INDEX idx_memories_superseded_by ON memories (superseded_by) WHERE superseded_by IS NOT NULL;
