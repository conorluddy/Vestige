---
name: vestige-trace-replay
description: 'Use this skill to re-run a stored trace against the current memory store and diff the results against the original — answering "would the agent get the same answer today?". Fire when the user says "re-run trace_X", "replay that search", "diff trace_X against current state", or "did the answers drift?". Read-only: writes a NEW query_events row tagged with replay_of; never mutates the original trace or any memory. provider_match=false flags embedding drift.'
---

# Replay a stored query trace

Re-run a stored query trace against the current store and current embedding provider. Produces an explicit diff — added/removed result IDs, score deltas — and flags whether the provider and corpus match the original. This is the "did memory drift?" command.

## When to fire

- **Drift detection.** After a major memory session (many records, approvals, or forgets) — confirm that prior recall results are stable.
- **Provider change.** Switching embedding providers or models — replay key traces to quantify the impact.
- **Corpus growth audit.** New memories were captured since a trace was written — replay shows what's been added to the result set.
- **Regression check.** Before a PR that touches recall logic — replay a set of traces and surface score deltas.

Replay is **read-only with respect to memories**. It writes a new `query_events` row tagged `caller=cli` (or `mcp`) with `replay_of: <original_trace_id>` in `params_json`, making the replay chain inspectable. The original trace is never modified.

## How to invoke

```bash
vestige trace replay <trace_id> [--json]
```

- Accepts a `trace_<ULID>` — obtain one from `vestige trace` (list mode).
- `provider_match=false` in the output signals that the current embedding provider or model differs from the one in the original trace. When this happens the mode may fall back to lexical and `mode_fallback=true` is set.

```bash
vestige trace replay trace_01JWXXXXXXXXXXXXXXXXXX           # text diff
vestige trace replay trace_01JWXXXXXXXXXXXXXXXXXX --json    # JSON diff
```

## After invocation

```json
{
  "trace_id": "trace_01JWXXXXXXXXXXXXXXXXXX",
  "original": {
    "result_ids": ["mem_01JWXXXXXXXXXXXXXXXXXX", "mem_01HVXXXXXXXXXXXXXXXXXX"],
    "scores":     [0.83, 0.61]
  },
  "current": {
    "result_ids": ["mem_01JWXXXXXXXXXXXXXXXXXX", "mem_01HVXXXXXXXXXXXXXXXXXX", "mem_01KAXXXXXXXXXXXXXXXXXX"],
    "scores":     [0.83, 0.61, 0.55]
  },
  "diff": {
    "added":         ["mem_01KAXXXXXXXXXXXXXXXXXX"],
    "removed":       [],
    "score_changes": []
  },
  "provider_match":  true,
  "mode_fallback":   false,
  "corpus_drift":    1,
  "replay_trace_id": "trace_01KBXXXXXXXXXXXXXXXXXX"
}
```

`corpus_drift` is the net change in result count. `replay_trace_id` is the ID of the new `query_events` row written for this replay run — inspect it with `vestige-trace-show <replay_trace_id>`.
