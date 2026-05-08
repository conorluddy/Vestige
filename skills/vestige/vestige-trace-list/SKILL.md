---
name: vestige-trace-list
description: 'Use this skill when you want to audit what queries the agent or CLI has run against project memory. Fire when the user says "what queries did the agent run?", "show recent trace events", "did anyone search for X recently?", or "list traces by caller mcp". Every vestige search, recall, expand, and context call writes a query_events row automatically — this skill surfaces them. Use vestige-trace-show to expand a single trace.'
---

# List recent query traces

Every `vestige search`, `vestige recall`, `vestige expand`, and `vestige context` call writes a row to `query_events` automatically — no configuration required. This skill lists those rows, filtered by kind, caller, date, or count. Use it to audit what the agent asked and when.

## When to fire

- **Audit trail.** Reviewing what recall calls were made in a session before writing a summary or PR.
- **Debug recall quality.** Seeing what mode (lexical / hybrid) was resolved for recent searches.
- **MCP vs CLI split.** Filtering by `--caller` to separate agent-driven traces from human CLI runs.
- **Temporal filter.** Narrowing to traces since a specific date — e.g. since a refactor started.

## How to invoke

```bash
vestige trace [--limit N] [--kind <kind>] [--caller <cli|mcp>] [--since <date>] [--json]
```

- **`--limit <n>`**: number of traces to return (default: 10).
- **`--kind <kind>`**: filter by trace kind — `search`, `expand`, or `context`.
- **`--caller <cli|mcp>`**: filter by surface that issued the query.
- **`--since <date>`**: only traces at or after this ISO-8601 date/datetime.
- **`--json`**: structured output.

```bash
vestige trace                            # last 10 traces, text
vestige trace --limit 50                 # last 50
vestige trace --kind search              # search traces only
vestige trace --caller mcp               # agent-originated only
vestige trace --since 2026-05-08         # since date
vestige trace --json                     # JSON envelope
```

## After invocation

```json
{
  "traces": [
    {
      "trace_id":    "trace_01JWXXXXXXXXXXXXXXXXXX",
      "kind":        "search",
      "mode":        "hybrid",
      "query":       "ULID migration ordering",
      "result_count": 2,
      "latency_ms":  43,
      "caller":      "mcp",
      "created_at":  "2026-05-08T14:02:11Z"
    },
    {
      "trace_id":    "trace_01JVXXXXXXXXXXXXXXXXXX",
      "kind":        "expand",
      "mode":        null,
      "query":       null,
      "result_count": 1,
      "latency_ms":  3,
      "caller":      "mcp",
      "created_at":  "2026-05-08T14:01:58Z"
    }
  ]
}
```

Use `vestige-trace-show <trace_id>` to expand a single trace to its full detail including result IDs and score breakdown.
