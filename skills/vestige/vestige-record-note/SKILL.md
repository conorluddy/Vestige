---
name: vestige-record-note
description: Capture a general project note to Vestige memory when you learn or surface a non-trivial fact about the codebase, its setup, an external service's quirk, a workaround for a tooling bug, a non-obvious gotcha, or anything future-you would want to remember but that isn't a decision (no firm commitment), a preference (no user opinion), or an open question (the answer is known). Use this skill when you say or hear "good to know:", "TIL", "note that…", "worth remembering:", "heads up:", "FYI:", "useful fact:", or when the user says "jot this down", "note that down", "remember this fact", or describes something they want captured without committing to it. Default importance is 0.5; the JSON envelope returns the new memory's handle (`mem_<ULID>`) and a compact card.
---

# Record a project note

Use Vestige to capture useful project knowledge that doesn't fit a more specific type. Notes are the catch-all bucket — lower-signal than decisions but still worth keeping.

## When to fire

- You discovered a non-obvious thing about the codebase ("`cargo test --workspace` triggers FTS triggers in tmpdir-shaped tests").
- You found a workaround for a tool quirk.
- You learned a fact that's true *now* but not necessarily a project commitment.
- The user said "jot this down" / "note that" / "worth recording" without naming it a decision.

If the moment carries a *commitment* with a *why*, use `vestige-record-decision` instead. If it's a user *opinion* about how the project should be done, use `vestige-record-preference`. If it's an unanswered *question*, use `vestige-record-question`.

## How to capture

```bash
vestige note add "<the fact, in one or two sentences>" \
  --importance 0.5 \
  --json
```

- **body** (positional, required): the fact itself, written so it stands alone — assume future-you reads it cold.
- **`--importance`** (optional): default 0.5. Drop to 0.3 for trivia; raise to 0.7 only if losing this would actually hurt.
- **`--source`** (optional): when the note came from a specific file/PR/URL, attach it. ≤ 2 KiB.

## After capture

Read the returned `id` and surface a one-liner: *"Noted `mem_…`."* — no need to read the body back.

## When NOT to capture

- The user already said it once and you can paraphrase it back. Don't note conversational acknowledgements.
- The fact is fully encoded in code (a comment in the source serves better).
- You're tempted to record a question. Use `vestige-record-question` — open questions get their own surfacing in `vestige context`.
