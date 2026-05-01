---
name: vestige-forget
description: Soft-delete a Vestige memory by its handle (`mem_<ULID>`) when the memory is wrong, superseded, or stale. Fire when a decision has been reversed, a note is no longer accurate, an open question has been answered, or the user says "forget memory <id>", "that's outdated", "remove <id>", "drop that memory", "we don't do that anymore". Flips status to `deleted` and drops the memory from search results — the row stays in the journal. Reversible via `vestige-restore`.
---

# Soft-delete a memory

Mark a memory as superseded so it stops surfacing in `vestige-recall`, `vestige-context`, and search. The memory itself stays in the durable journal — `forget` is reversible.

## When to fire

- A previously-recorded decision has been **reversed**. Capture the new decision with `vestige-record-decision`, then `vestige-forget` the old one.
- A previously-recorded note is **no longer accurate**. Either capture an updated note first, or just forget the stale one.
- An open question has been **answered**. Record the answer as a decision, then forget the question.
- The user explicitly says **"forget X"** or **"that's outdated"**.

Do NOT use this for routine cleanup. Forgetting too aggressively makes the project's history less useful for future-you. The default state of a memory is "kept forever".

## How to invoke

```bash
vestige forget <mem_id> --json
```

- **`<mem_id>`** (positional, required): the handle of the memory to forget. Exact match — no partials or globs.
- **`--json`** (optional): structured envelope so the agent can parse the result.

When forgetting because of supersession, capture the replacement first:

```bash
vestige decision add "Use Postgres instead of SQLite" --rationale "…" --json
# → returns mem_<NEW>
vestige forget mem_<OLD> --json
```

That way the journal records both the supersession event and the new commitment.

## After invocation

Surface the action briefly: *"Forgot `mem_…` (soft-delete; restorable with `vestige restore`)."* — explicit so the user knows it's reversible.

If you forgot a memory in error, fire `vestige-restore` immediately.

## Idempotence & dedup

Soft-delete is idempotent in spirit: forgetting an already-deleted memory is a no-op (non-zero exit). The underlying row is never `DELETE`d — Vestige's hard rule is soft-delete only. Restore re-flips the status.
