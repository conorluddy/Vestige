---
name: vestige-sources
description: 'Use this skill when you need the raw source receipts for a memory or candidate — the typed evidence rows, not the narrative walk. Fire when the user says "what sources back mem_X?", "show me the source receipts", "list the sources for mem_X", or "filter sources by kind agent_session". Use `vestige-why` when you want the full provenance narrative including events; use this skill when you want the tabular source index only.'
---

# List source receipts

Return the typed source rows attached to a memory or candidate. Every memory captured with `--source` or promoted from a candidate carries at least one receipt. This skill gives you the raw evidence index — kind, reference, and optional content snippet — without the event narrative.

## When to fire

- **Evidence check.** Confirming what files, commits, or sessions back a memory before citing it.
- **Kind filtering.** Narrowing to a specific source type — e.g. only `agent_session` sources, or only `file` sources.
- **Pre-approval audit.** Inspecting a candidate's sources before deciding whether to approve or reject.
- **Bulk source review.** A memory has many sources and you want the table, not the prose.

## How to invoke

```bash
vestige sources <mem_or_cand_id> --json
```

- **`--kind <kind>`**: filter to one source kind. Valid kinds: `file`, `commit`, `url`, `agent_session`, `mcp_call`, `candidate`, `manual`, `trace`.
- Accepts `mem_<ULID>` or `cand_<ULID>`.

```bash
vestige sources mem_01JWXXXXXXXXXXXXXXXXXX                       # all sources, text
vestige sources mem_01JWXXXXXXXXXXXXXXXXXX --kind agent_session  # filtered
vestige sources cand_01JVXXXXXXXXXXXXXXXXXX                      # candidate sources
vestige sources mem_01JWXXXXXXXXXXXXXXXXXX --json                # JSON output
```

## After invocation

```json
{
  "owner_id": "mem_01JWXXXXXXXXXXXXXXXXXX",
  "sources": [
    {
      "id":         "src_01JWXXXXXXXXXXXXXXXXXX",
      "kind":       "candidate",
      "source_ref": "cand_01JVXXXXXXXXXXXXXXXXXX",
      "content":    null,
      "truncated":  false
    },
    {
      "id":         "src_01JVXXXXXXXXXXXXXXXXXX",
      "kind":       "agent_session",
      "source_ref": "current",
      "content":    "Use ULID over UUID for all entity IDs — lexicographic sort…",
      "truncated":  false
    }
  ]
}
```

Source kinds: `file` · `commit` · `url` · `agent_session` · `mcp_call` · `candidate` · `manual` · `trace`. Use `vestige-why <id>` for the full event walk alongside sources.
