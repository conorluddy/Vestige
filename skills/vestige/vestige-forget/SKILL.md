---
name: vestige-forget
description: Soft-delete a Vestige memory by its handle (`mem_<ULID>`) when the memory is wrong, superseded by a newer decision, no longer relevant, or contains stale information. Fire when a previously-recorded decision has been reversed, a note is no longer accurate, an open question has been answered (and a new decision recorded in its place), or the user says "forget memory <id>", "that's outdated", "remove <id>", "drop that memory", "we don't do that anymore". Soft-delete only — the row stays in the journal and `memory_events`, just flips status to `deleted` and drops out of search results. Reversible via `vestige-restore`.
---

# Soft-delete a memory

Mark a memory as superseded so it stops surfacing in `vestige-recall`, `vestige-context`, and search. The memory itself stays in the durable journal — `forget` is reversible.

## When to fire

- A previously-recorded decision has been **reversed**. Capture the new decision with `vestige-record-decision`, then `vestige-forget` the old one.
- A previously-recorded note is **no longer accurate**. Either capture an updated note first, or just forget the stale one.
- An open question has been **answered**. Record the answer as a decision, then forget the question.
- The user explicitly says **"forget X"** or **"that's outdated"**.

Do NOT use this for routine cleanup. Forgetting too aggressively makes the project's history less useful for future-you. The default state of a memory is "kept forever".

## How to forget

```bash
vestige forget <mem_id>
```

- **`<mem_id>`** (positional, required): the handle of the memory to forget. Exact match — no partials or globs.

There's no `--json` output to parse — exit code 0 means success, non-zero means the id wasn't found or was already deleted.

## After forgetting

Surface the action briefly: *"Forgot `mem_…` (soft-delete; restorable with `vestige restore`)."* — explicit so the user knows it's reversible.

If you forgot a memory in error, fire `vestige-restore` immediately.

## Linked actions

When forgetting because of supersession, capture the replacement first:

```bash
# 1. Record the new decision.
vestige decision add "Use Postgres instead of SQLite" --rationale "…" --json
# → returns mem_<NEW>

# 2. Forget the old one.
vestige forget mem_<OLD>
```

This way the journal records both the supersession event and the new commitment.
