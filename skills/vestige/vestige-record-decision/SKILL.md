---
name: vestige-record-decision
description: 'Capture a project decision to Vestige memory when committing to an architectural choice, picking approach A over B, settling a tradeoff, or choosing a library / framework / pattern. Fire on "we''ll go with…", "let''s use…", "decision:…", "I''m choosing X over Y because…", "settled — we use…", "going with X", or when the user says "capture that decision", "remember this", "record it". Decisions ground future reasoning and feed every `vestige context` pack. Captures body, rationale (the why), and importance (default 0.7). Returns the new handle (`mem_<ULID>`).'
---

# Record a project decision

Write a project decision into durable, repo-scoped Vestige memory. Decisions are commitments with a why — the kind of thing future-you would regret losing.

## When to fire

- The conversation has just produced a commitment to a specific approach, library, pattern, or architectural shape.
- The user said "we'll go with X" / "decision: X" / "let's use X" / "settled".
- You wrote `## Decision` or `Decision:` in your own response.
- You explicitly chose A over B and gave a reason.

Tie-breakers vs siblings:

- *Decision vs note* — does it carry a commitment **and** a why? If both yes, decision. If just useful info, use `vestige-record-note`.
- *Decision vs preference* — preferences come from the user's voice ("I prefer …", "always …"). Decisions can come from either side and have an explicit rationale.
- *Decision vs question* — if there's still ambiguity, the answer isn't a decision yet. Use `vestige-record-question`.

## How to invoke

```bash
vestige decision add "<one-line decision>" \
  --rationale "<the why; cite tradeoffs, alternatives, constraints>" \
  --importance 0.7 \
  --json
```

- **body** (positional, required): one-line statement of what was decided. Imperative or declarative ("Use SQLite for local persistence", "Adopt FTS5 for lexical search").
- **`--rationale`** (recommended): the why. Without rationale a decision degrades to a note.
- **`--importance`** (optional, 0.0–1.0, default 0.7): bump to 0.85+ for decisions that shape major surface area; leave at default for routine choices.
- **`--source`** (optional): file path or external ref (`crates/vestige-store/src/lib.rs:42`, `https://...`) when the decision was prompted by something inspectable.
- **`--source-content`** (optional): inline snippet, capped at 2 KiB by the CLI.

## After invocation

The JSON envelope returns `{ "id": "mem_<ULID>", ... }`. Read `id` and surface a one-line confirmation: *"Recorded decision `mem_…`."* Don't read the body back — the user just said it.

The decision is immediately searchable via `vestige-recall` and will appear in the next `vestige context` pack.

## Idempotence & dedup

Every `vestige decision add` is a fresh write — there is no upsert. Before capturing, run a quick dedup probe if you suspect overlap:

```bash
vestige recall "<key phrase from the decision>" --json --limit 3
```

If the top hit's `score` is high *and* `type` is `decision` *and* the `one_liner` is essentially the same commitment, skip the write and cite the existing handle instead. Otherwise capture; soft-delete (`vestige-forget`) the prior one if it's been superseded.
