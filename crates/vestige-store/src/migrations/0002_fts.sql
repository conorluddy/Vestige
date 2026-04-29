CREATE VIRTUAL TABLE memory_fts USING fts5(
    memory_id           UNINDEXED,
    representation_type UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);

CREATE TRIGGER memory_repr_after_insert
AFTER INSERT ON memory_representations
BEGIN
    INSERT INTO memory_fts (memory_id, representation_type, content)
    VALUES (NEW.memory_id, NEW.representation_type, NEW.content);
END;

CREATE TRIGGER memory_repr_after_update
AFTER UPDATE ON memory_representations
BEGIN
    DELETE FROM memory_fts WHERE memory_id = OLD.memory_id AND representation_type = OLD.representation_type;
    INSERT INTO memory_fts (memory_id, representation_type, content)
    VALUES (NEW.memory_id, NEW.representation_type, NEW.content);
END;

CREATE TRIGGER memory_repr_after_delete
AFTER DELETE ON memory_representations
BEGIN
    DELETE FROM memory_fts WHERE memory_id = OLD.memory_id AND representation_type = OLD.representation_type;
END;

-- When a memory is soft-deleted, drop its FTS rows so it falls out of search.
CREATE TRIGGER memory_after_soft_delete
AFTER UPDATE OF status ON memories
WHEN NEW.status = 'deleted' AND OLD.status <> 'deleted'
BEGIN
    DELETE FROM memory_fts WHERE memory_id = NEW.id;
END;

-- Restoring a memory re-indexes its representations.
CREATE TRIGGER memory_after_restore
AFTER UPDATE OF status ON memories
WHEN NEW.status = 'active' AND OLD.status = 'deleted'
BEGIN
    INSERT INTO memory_fts (memory_id, representation_type, content)
    SELECT memory_id, representation_type, content
    FROM memory_representations
    WHERE memory_id = NEW.id;
END;
