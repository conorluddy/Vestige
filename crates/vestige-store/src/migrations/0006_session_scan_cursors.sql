CREATE TABLE session_scan_cursors (
    source          TEXT NOT NULL,           -- "claude_code" | "codex"
    file_path       TEXT NOT NULL,
    project_id      TEXT NOT NULL,
    last_offset     INTEGER NOT NULL,        -- byte offset (or line) scanned through
    last_scanned_at TEXT NOT NULL,           -- RFC-3339 UTC (house rule: timestamps are TEXT)
    PRIMARY KEY (source, file_path)
);
