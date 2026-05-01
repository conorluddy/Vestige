---
name: vestige-context
description: Render the project's Vestige context pack — summary, recent decisions, open questions, recent activity — as a budget-bounded brief. Use at session start, before unfamiliar work, after a long pause, or when the user says "what's the state of this project?", "where are we?", "catch me up", "what have we decided?", "what's outstanding?", "give me the rundown". Returns a JSON envelope with `summary`, `decisions[]`, `open_questions[]`, `recent[]`. Token budget defaults to 1200 — pass `--budget-tokens` to adjust.
---

# Get the project context pack

Pull the durable Vestige memory an agent should read before doing serious work in this project. Same shape MCP tooling exposes via `vestige_get_project_context`.

## When to fire

- **Session start.** First time you're working on this project today, fire this once.
- **Cold start on unfamiliar code.** About to refactor a module you haven't touched? Pull the pack first.
- **User explicitly asks.** "What's the state?", "remind me where we are", "summarise this project for me".
- **Before a non-trivial decision.** If you're about to commit to architecture, the pack tells you what you're already committed to.

Tie-breaker vs `vestige-recall`: the pack is *broad* (project-wide). Recall is *narrow* (one query). If you have a specific question, use recall — it's cheaper.

## How to invoke

```bash
vestige context --json
```

- **`--budget-tokens <n>`** (default 1200): caps the total pack size. Lower it (300–500) when context is tight; raise it (2400+) for a deep dive.
- **`--per-section <n>`** (default 8): caps each list section.
- **`--json`** (recommended for agents): structured envelope. Skip for human-readable markdown.

## After invocation

The JSON envelope:

```json
{
  "project_name": "…",
  "summary": { "id": "mem_…", "one_liner": "…", "available_depths": […] } | null,
  "decisions": [{ "id": "mem_…", "one_liner": "…", "score": …, "type": "decision" }, …],
  "open_questions": [ … ],
  "recent": [ … ],
  "warnings": []
}
```

For any card whose `one_liner` isn't enough, use `vestige-show <id>` to expand at higher fidelity. Cite handles (`mem_…`) when you reference a finding.

## Idempotence & dedup

Read-only — calling repeatedly is safe and cheap, but pointless. Skip if:

- You already pulled the pack this session and nothing has changed.
- The user just asked a narrow question — don't pull the whole pack to answer "what's our SQLite version?", use `vestige-recall "sqlite"` instead.
