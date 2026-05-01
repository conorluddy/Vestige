---
name: vestige-record-decision
description: Capture a project decision to Vestige memory whenever the model commits to an architectural choice, picks approach A over B, settles a tradeoff, chooses a library / framework / pattern, or answers a "how should we do X?" question. Use this skill when you say or hear things like "we'll go with…", "let's use…", "decision:…", "I'm choosing X over Y because…", "settled — we use…", "going with X", or when the user says "capture that decision", "remember this", "record it", or names something they just decided. Captures the decision body, a rationale (the *why*), and an optional importance score (default 0.7 — decisions are higher-signal than notes). Returns the new memory's handle (`mem_<ULID>`) plus a compact card the agent can quote in subsequent reasoning.
---

# Record a project decision

Use Vestige to write a project decision into durable, repo-scoped memory. Decisions are the highest-signal memory type — they ground future reasoning ("we already chose SQLite, don't re-litigate") and feed every `vestige context` pack.

## When to fire

- The conversation has just produced a commitment to a specific approach, library, pattern, or architectural shape.
- The user said "we'll go with X" / "decision: X" / "let's use X" / "settled".
- You wrote `## Decision` or `Decision:` in your own response.
- You explicitly chose A over B and gave a reason.

If you're unsure whether something is a decision vs a note, ask: *would future-me regret losing this?* and *is there a clear reason behind it?* If both yes → decision. If just "useful info" → use `vestige-record-note` instead.

## How to capture

Always shell out via Bash and pass `--json` so the output is parseable.

```bash
vestige decision add "<one-line decision>" \
  --rationale "<the why; cite tradeoffs, alternatives, constraints>" \
  --importance 0.7 \
  --json
```

Argument guidance:

- **body** (positional, required): one-line statement of what was decided. Imperative or declarative ("Use SQLite for local persistence", "Adopt FTS5 for lexical search").
- **`--rationale`** (recommended): the why. This is what makes a decision worth recording — a body without rationale degrades to a note.
- **`--importance`** (optional, 0.0–1.0, default 0.7): bump to 0.85+ for decisions that shape major surface area; leave at default for routine choices.
- **`--source`** (optional): file path or external ref (`crates/vestige-store/src/lib.rs:42`, `https://...`) when the decision was prompted by something inspectable. Capped at 2 KiB.

## After capture

- Read the returned `id` (handle of the form `mem_<ULID>`).
- Surface it to the user: *"Recorded decision `mem_…`."* — short and uncoloured. Don't read the body back; they just said it.
- The decision is immediately searchable via `vestige-recall` and will appear in the next `vestige context` pack.

## Idempotence

Every `vestige decision add` call writes a new row. If you've already captured a decision verbatim, don't re-capture it — call `vestige-recall "<key phrase>"` first if you suspect a duplicate.
