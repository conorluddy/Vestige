---
name: vestige-show
description: Expand a Vestige memory at higher fidelity once you have its handle (a `mem_<ULID>` string). Use this skill after `vestige-recall` returns a card whose `one_liner` isn't enough, when the user says "show me memory <id>", "expand <id>", "what does <id> say in full?", "read me memory <id>", or whenever you need the *body*, *rationale*, or *source content* of a memory rather than the compact handle. Depth ladder: one_liner → summary → compressed → full. Default is summary; pass `--depth full` to read the entire body.
---

# Expand a memory by handle

Read a memory at the depth you actually need. Vestige stores memories as a depth ladder (PRD §11.3): `one_liner` (≤ 60 chars), `summary` (a sentence), `compressed` (a paragraph), `full` (the whole body). `vestige-recall` returns the compact card — this skill drills in.

## When to fire

- `vestige-recall` just returned a card whose one-liner sounds relevant but you need to read the actual decision/rationale to apply it.
- The user said "show me `mem_…`" or "what does `mem_…` say".
- You're about to cite a memory and want to confirm the wording.

If you don't have a handle yet, run `vestige-recall` first.

## How to fetch

```bash
vestige show <mem_id> --depth full --json
```

- **`<mem_id>`** (positional, required): the `mem_<ULID>` handle from a recall hit or a context pack.
- **`--depth`** (optional, default `summary`): one of `one_liner`, `summary`, `compressed`, `full`. Use `full` when you need everything; otherwise stick to `summary` to save tokens.
- **`--json`** (recommended): structured envelope with `id`, `type`, `depth`, `content`, `sources[]`.

## Token discipline

Memories can be large (full body up to a few KB; source snippets capped at 2 KiB). Don't reach for `--depth full` reflexively — `summary` is usually enough to apply a decision, and the depth ladder exists precisely to keep agent context lean.

## When to skip

- You only need the one-liner the recall already returned. Don't re-fetch.
- You want a *list* of memories, not one. Use `vestige-recall` with `--limit`.
