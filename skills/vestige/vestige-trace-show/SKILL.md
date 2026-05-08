---
name: vestige-trace-show
description: 'Use this skill when you want to expand a specific trace to its full detail — the query text, mode requested vs resolved, provider, result IDs, and scores. Fire when the user says "expand trace_X", "show me what trace_X returned", or "what did that search find?". Use vestige-trace-list first to find the trace_id, then this skill to inspect one in full.'
---

# Expand a single query trace

Return the full detail for one `query_events` row: kind, mode (requested vs resolved), provider, parameters, result IDs with scores, and score methodology. Use this after `vestige-trace-list` to drill into a specific trace.

## When to fire

- **Inspect a recall result set.** "What did that hybrid search find?" → expand the trace to see IDs and scores.
- **Debug mode resolution.** Confirm whether `hybrid` was requested and resolved, or fell back to lexical.
- **Provider audit.** Check which embedding model was in use when a trace was written.
- **Score breakdown.** Understand how results were ranked (RRF fusion, vector similarity, or FTS).

## How to invoke

```bash
vestige trace <trace_id> [--json]
```

- Accepts a `trace_<ULID>` — obtain one from `vestige trace` (list mode) or the `trace_id` field in any trace envelope.

```bash
vestige trace trace_01JWXXXXXXXXXXXXXXXXXX           # text output
vestige trace trace_01JWXXXXXXXXXXXXXXXXXX --json    # JSON output
```

## After invocation

```json
{
  "trace_id":       "trace_01JWXXXXXXXXXXXXXXXXXX",
  "kind":           "search",
  "mode_requested": "hybrid",
  "mode_resolved":  "hybrid",
  "caller":         "mcp",
  "query":          "ULID migration ordering",
  "provider":       "fake",
  "provider_model": "deterministic-sha256",
  "params": {
    "limit": 8,
    "type_filter": null
  },
  "result_count":   2,
  "result_ids":     ["mem_01JWXXXXXXXXXXXXXXXXXX", "mem_01HVXXXXXXXXXXXXXXXXXX"],
  "result_scores":  [0.83, 0.61],
  "latency_ms":     43,
  "created_at":     "2026-05-08T14:02:11Z"
}
```

To re-run a trace against the current store and diff the results, use `vestige-trace-replay <trace_id>`.
