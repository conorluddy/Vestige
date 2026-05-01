---
name: vestige-restore
description: 'Restore a previously soft-deleted Vestige memory by its handle (`mem_<ULID>`). Use when a memory was forgotten in error, when the situation has reverted (a decision that was reversed gets re-affirmed), or the user says "bring back <id>", "restore <id>", "undo that forget", "that memory should still be there". Re-flips status from `deleted` to `active` and re-indexes the memory''s representations into FTS. Note — embeddings are left stale by restore (they''ll re-embed on the next `vestige embed` run).'
---

# Restore a soft-deleted memory

Undo a `vestige-forget` by flipping the memory's status back to `active`.

## When to fire

- A memory was forgotten in error.
- A decision that was reversed has been re-affirmed (the new "decision" is "we changed our mind back").
- The user says **"restore <id>"** / **"bring back <id>"** / **"undo that forget"**.

If the memory was never recorded in the first place, capture it via `vestige-record-*` instead — restore only works on previously soft-deleted rows.

## How to invoke

```bash
vestige restore <mem_id> --json
```

- **`<mem_id>`** (positional, required): the handle of a previously soft-deleted memory.
- **`--json`** (optional): structured envelope.

Exit code 0 means restored. Non-zero means the id wasn't found (or wasn't in `deleted` state — `restore` only acts on soft-deleted rows).

## After invocation

Surface: *"Restored `mem_…`. It's back in search results."* The memory is immediately searchable via `vestige-recall` again.

Note on embeddings: restore re-indexes FTS triggers automatically, but does **not** regenerate embeddings — the old embeddings stay marked stale. If semantic recall matters for the restored memory, suggest the user run `vestige embed --all` to re-embed.

## Idempotence & dedup

Restore is idempotent: calling it on an already-active memory is a no-op (non-zero exit). Safe to retry.
