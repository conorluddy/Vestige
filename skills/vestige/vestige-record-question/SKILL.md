---
name: vestige-record-question
description: Capture an open question to Vestige memory when an ambiguity is identified that cannot be resolved right now and will need a human, an investigation, or a later session to answer. Fire when you say or hear "TBD:", "open question:", "we should figure out…", "unclear whether…", "need to decide later if…", "??:", "?:", "follow-up:", "to investigate:", "leaving X open", or when the user says "capture that as a question", "we'll come back to that", "park that for now", "good question — note it down". Default importance is 0.5; the question gets surfaced in every `vestige context` pack until it's `vestige-forget`'d (typically when answered, often paired with a `vestige-record-decision`).
---

# Record an open question

Capture an unresolved question / ambiguity / TBD so it isn't lost between sessions. Questions surface in the project context pack so future-you (or a future agent) can either answer them or rule them out.

## When to fire

- A real ambiguity surfaced — the answer matters but isn't available now.
- You're tempted to write "TODO" or "FIXME" but it's a *design* question, not a code task.
- The user said "let's come back to that" / "park it" / "we'll figure that out later".
- You wrote `## Open Questions` or `### TBD` in your own response.

If the question is purely a code task (write the function, fix the bug), it doesn't belong here — leave it as a TODO comment in the source. Memories are for *project-level* ambiguities.

## How to capture

```bash
vestige question add "<the question, framed as a question>" \
  --importance 0.5 \
  --json
```

- **body** (positional, required): write it as an actual question. "Should we keep V0 single-process or move to a daemon?" beats "Daemon mode TBD".
- **`--importance`** (optional): default 0.5. Bump to 0.8+ for blockers; drop to 0.3 for "nice to know eventually".
- **`--source`** (optional): when the question was prompted by a specific file or piece of context, attach it.

## After capture

Read the returned `id`. Surface: *"Captured open question `mem_…`. It'll appear in `vestige context` until resolved."*

## Lifecycle

When the question gets answered:

1. Use `vestige-record-decision` to capture the answer.
2. Use `vestige-forget` on the original question's handle so the context pack stops surfacing it.

(The journal still has the original question — `forget` is soft.)
