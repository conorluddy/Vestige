---
name: vestige-context
description: Render the project's Vestige context pack — project summary, recent decisions, open questions, recent activity — as a budget-bounded markdown brief. Use this skill at session start, right before tackling unfamiliar work in this codebase, after a long pause from this project, or when the user asks "what's the state of this project?", "where are we?", "catch me up", "what have we decided?", "what's outstanding?", "give me the rundown". Returns a compact JSON envelope (or markdown text) with the project_summary memory, the N most-important decisions, the open questions still in flight, and a tail of recent activity. Token budget defaults to 1200 — passing `--budget-tokens` tightens or loosens.
---

# Get the project context pack

Pull the durable Vestige memory that an agent should read before doing serious work in this project. The pack is constructed by `vestige-core::build_pack` and is the same shape MCP tooling exposes via `vestige_get_project_context`.

## When to fire

- **Session start.** First time you're working on this project today, fire this once.
- **Cold start on unfamiliar code.** About to refactor a module you haven't touched? Pull the pack first.
- **User explicitly asks.** "What's the state?", "remind me where we are", "summarise this project for me".
- **Before a non-trivial decision.** If you're about to commit to architecture, the pack tells you what you're already committed to.

If you're searching for a *specific* piece of memory (a decision about X), use `vestige-recall` instead — narrower, cheaper.

## How to fetch

```bash
vestige context --json
```

Useful flags:

- **`--budget-tokens <n>`** (default 1200): caps the total pack size. Lower it (300–500) when you're tight on context window; raise it (2400+) for a deep dive.
- **`--json`** (recommended for skills): structured envelope with `summary`, `decisions[]`, `open_questions[]`, `recent[]` sections. Skip for human-readable markdown.

## After fetching

Read the JSON. The structure:

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

Use `available_depths` and `vestige-show <id>` to expand any card you need at higher fidelity.

## When to skip

- The user just asked a narrow question. Don't pull the whole pack to answer "what's our SQLite version?" — `vestige-recall "sqlite"` is the right tool.
- You already pulled the pack this session and nothing has changed.
