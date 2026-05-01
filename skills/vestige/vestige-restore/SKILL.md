---
name: vestige-restore
description: Restore a previously soft-deleted Vestige memory by its handle (`mem_<ULID>`). Use when a memory was forgotten in error, when the situation has reverted (a decision that was reversed gets re-affirmed), or the user says "bring back <id>", "restore <id>", "undo that forget", "that memory should still be there". Re-flips status from `deleted` to `active` and re-indexes the memory's representations into FTS. Note: embeddings are left stale by restore (they'll re-embed on the next `vestige embed` run).
---

# Restore a soft-deleted memory

Undo a `vestige-forget` by flipping the memory's status back to `active`. Restore is idempotent — calling it on an already-active memory is a no-op.

## When to fire

- A memory was forgotten in error.
- A decision that was reversed has been re-affirmed (the new "decision" is "we changed our mind back").
- The user says **"restore <id>"** / **"bring back <id>"** / **"undo that forget"**.

## How to restore

```bash
vestige restore <mem_id>
```

- **`<mem_id>`** (positional, required): the handle of a previously soft-deleted memory.

Exit code 0 means restored. Non-zero means the id wasn't found (or wasn't in `deleted` state — `restore` only acts on soft-deleted rows).

## After restoring

Surface: *"Restored `mem_…`. It's back in search results."* The memory is immediately searchable via `vestige-recall` again.

## Note on embeddings

Restore re-indexes FTS triggers automatically, but does **not** regenerate embeddings — the old embeddings stay marked stale. If semantic recall matters for the restored memory, suggest the user run `vestige embed --all` to re-embed.

## When to skip

- The memory is already active. Restore is idempotent but pointless in that case.
- You're trying to restore a memory that was never recorded. Use `vestige-record-*` to create it instead.
