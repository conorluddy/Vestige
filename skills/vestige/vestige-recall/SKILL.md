---
name: vestige-recall
description: Search Vestige memory before a decision, refactor, or library choice — any time you suspect "we already discussed this". Fire when the user says "have we decided this?", "what did we say about X?", "look up our notes on Y", "is this already settled?", "search memory for Z". Lexical (BM25) by default; `--hybrid` for merged lexical + semantic recall (requires `vestige embed --all`); `--semantic` for cosine-only. Returns ranked compact cards with `mem_<ULID>` handles. Use `vestige-show <id>` to expand.
---

# Recall prior memory

Search the project's durable memory before grounding any new decision. Recall is the workhorse retrieval skill — every memory you've captured becomes useful only insofar as it's findable later.

## When to fire

- **Before committing.** About to choose X? Run `vestige-recall "X"` first to find prior decisions / notes / preferences.
- **Before refactoring.** Pull recall on the module name to surface decisions that constrain the shape.
- **When the user references prior discussion.** "We talked about this before" → recall, don't guess.
- **When you're tempted to ask the user a question** that might already be answered in memory.
- **As a dedup probe** before any `vestige-record-*` capture, to avoid double-writing.

Tie-breaker vs `vestige-context`: pack is broad, recall is narrow. If you want the full state, use the pack.

## How to invoke

```bash
vestige recall "<query>" --json
```

- **`--hybrid`**: merged lexical + semantic recall. Best quality when the project has run `vestige embed --all` and a real embedding provider is configured. Falls back to lexical with a warning if embeddings are missing.
- **`--limit <n>`**: default is the project's `[recall] max_results` (typically 8). Tighten for focused queries.
- **`--type <decision|note|preference|question>`**: narrow to one memory type.
- **`--score-parts`** (with `--hybrid`): include FTS / vector / importance / type-boost breakdown in the JSON for explainability.

Avoid `--semantic` alone — it has no fallback when embeddings aren't ready. Use `--hybrid` instead; it degrades gracefully.

## After invocation

```json
{
  "mode": "lexical" | "semantic" | "hybrid",
  "results": [
    { "id": "mem_…", "type": "decision", "title": "…", "one_liner": "…", "score": 0.65, "available_depths": ["one_liner", "summary", "compressed", "full"] },
    …
  ],
  "warnings": []
}
```

Always check `warnings` — a `hybrid → lexical` fallback message means semantic recall isn't ready and you should suggest `vestige embed --all` if depth matters.

For each relevant hit, decide whether `one_liner` is enough or you need to expand via `vestige-show <id> --depth full`. Cite the handle when you reference a finding.

## Idempotence & dedup

Read-only — safe to re-run. Vary the query phrasing on a miss before concluding nothing matches; lexical recall is keyword-sensitive, and `--hybrid` (when embeddings exist) catches paraphrases lexical misses.
