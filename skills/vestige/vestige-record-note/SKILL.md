---
name: vestige-record-note
description: Capture a general project note to Vestige memory for non-trivial facts, workarounds, gotchas, aha moments, TILs, code smells, or durable TODOs — anything worth keeping that isn't a decision, preference, or open question. Fire on "good to know:", "TIL", "note that…", "heads up:", "FYI:", "turns out…", "the reason X happens is…", "TODO:", "smell:", "careful — X", or when the user says "jot this down", "note that down", "remember this fact". Default importance 0.5. Returns the new handle (`mem_<ULID>`).
---

# Record a project note

Capture useful project knowledge that doesn't fit a more specific type. Notes are the catch-all bucket — lower-signal than decisions, but still worth keeping. Aha moments, TILs, gotchas, code smells, and durable TODOs all live here.

## When to fire

- You discovered a non-obvious thing about the codebase ("`cargo test --workspace` triggers FTS triggers in tmpdir-shaped tests").
- You found a workaround for a tool quirk.
- You hit an "aha" — surprising behaviour, broken assumption, root cause after debugging.
- You're flagging a code smell or refactor candidate that should outlive this session.
- The user said "jot this down" / "note that" / "worth recording" without naming it a decision.

Tie-breakers vs siblings:

- *Note vs decision* — does it carry a commitment with a why? If yes, `vestige-record-decision`.
- *Note vs preference* — is it a user opinion / convention? If yes, `vestige-record-preference`.
- *Note vs question* — is the answer known? If no, `vestige-record-question`.

## How to invoke

```bash
vestige note add "<the fact, in one or two sentences>" \
  --importance 0.5 \
  --json
```

- **body** (positional, required): the fact itself, written so it stands alone — assume future-you reads it cold.
- **`--importance`** (optional, default 0.5): drop to 0.3 for trivia; raise to 0.6 for code smells / root causes worth keeping in front of the agent; 0.7 only if losing this would actually hurt.
- **`--source`** (optional): when the note came from a specific file/PR/URL, attach it.
- **`--source-content`** (optional): inline snippet, capped at 2 KiB by the CLI.

## After invocation

The JSON envelope returns `{ "id": "mem_<ULID>", ... }`. Surface a one-liner: *"Noted `mem_…`."* No need to read the body back.

## Idempotence & dedup

Every call is a fresh write. Before capturing a note that feels familiar, dedup with:

```bash
vestige recall "<key phrase>" --json --limit 3
```

Skip the write if the top hit is the same fact at a comparable importance. If you're capturing because *something changed*, capture the new note and `vestige-forget` the stale one — don't leave both active.

Don't note conversational acknowledgements, things already in code (a comment serves better), or anything that's actually a decision/preference/question in disguise.
