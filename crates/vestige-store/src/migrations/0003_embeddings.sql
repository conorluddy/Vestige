-- Embedding metadata: one row per (representation, provider, model) triple.
-- Status lifecycle: active → stale → (re-embed) → active | failed.
CREATE TABLE memory_embeddings (
    id                  TEXT PRIMARY KEY,
    memory_id           TEXT NOT NULL,
    representation_id   TEXT NOT NULL,
    representation_type TEXT NOT NULL,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    dimensions          INTEGER NOT NULL,
    vector_hash         TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'active',
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    stale_at            TEXT,
    FOREIGN KEY (memory_id)         REFERENCES memories(id),
    FOREIGN KEY (representation_id) REFERENCES memory_representations(id)
);

CREATE INDEX idx_embeddings_memory_status
    ON memory_embeddings (memory_id, status);

-- Unique: only one embedding per (representation, provider, model).
-- PR3: INSERT OR REPLACE (or UPDATE) when re-embedding the same representation.
CREATE UNIQUE INDEX idx_embeddings_repr_provider_model
    ON memory_embeddings (representation_id, provider, model);

-- Embedding job log: tracks every embedding attempt, successful or not.
-- Statuses: pending | completed | failed | skipped.
CREATE TABLE embedding_jobs (
    id                  TEXT PRIMARY KEY,
    memory_id           TEXT NOT NULL,
    representation_id   TEXT NOT NULL,
    representation_type TEXT NOT NULL,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    status              TEXT NOT NULL,
    error               TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    FOREIGN KEY (memory_id)         REFERENCES memories(id),
    FOREIGN KEY (representation_id) REFERENCES memory_representations(id)
);

CREATE INDEX idx_embedding_jobs_status_updated
    ON embedding_jobs (status, updated_at);

-- Vector storage: raw float bytes, one row per embedding.
-- Keyed 1:1 to memory_embeddings; cascades on delete.
-- sqlite-vec virtual table is NOT added here — that is PR3's responsibility.
CREATE TABLE memory_vectors (
    embedding_id    TEXT PRIMARY KEY,
    dimensions      INTEGER NOT NULL,
    vector          BLOB NOT NULL,
    FOREIGN KEY (embedding_id) REFERENCES memory_embeddings(id) ON DELETE CASCADE
);

-- When a representation's content changes, mark all its embeddings stale.
-- Does not delete; staleness is a recoverable marker. PR4 handles re-embedding.
CREATE TRIGGER embedding_repr_content_changed
AFTER UPDATE OF content_hash ON memory_representations
BEGIN
    UPDATE memory_embeddings
    SET    status   = 'stale',
           stale_at = CURRENT_TIMESTAMP,
           updated_at = CURRENT_TIMESTAMP
    WHERE  representation_id = NEW.id
      AND  status <> 'stale';
END;

-- When a memory is soft-deleted, cascade-mark all its embeddings stale.
-- Restore intentionally leaves them stale — they re-embed on demand (PRD §8.4).
CREATE TRIGGER embedding_memory_soft_deleted
AFTER UPDATE OF status ON memories
WHEN NEW.status = 'deleted' AND OLD.status <> 'deleted'
BEGIN
    UPDATE memory_embeddings
    SET    status    = 'stale',
           stale_at  = CURRENT_TIMESTAMP,
           updated_at = CURRENT_TIMESTAMP
    WHERE  memory_id = NEW.id
      AND  status <> 'stale';
END;
