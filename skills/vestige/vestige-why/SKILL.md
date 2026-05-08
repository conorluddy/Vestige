---
name: vestige-why
description: 'Use this skill when you want to know where a memory came from, need to explain its provenance, or must audit what was captured and when. Fire when the user says "where did this come from?", "explain that memory", "why do we have mem_X?", "show provenance for X", "what''s the source of mem_X?", or "trace this memory back". Works for both memories and pre-approval candidates. Soft-deleted memories are included — provenance is always inspectable.'
---

# Inspect memory provenance

Walk the full provenance chain of a memory or candidate: the raw `memory_events` journal, any candidate it was promoted from, and the source receipts attached at capture. This is the audit command — it answers "where did this come from and who put it there?".

## When to fire

- **Suspicious recall hit.** A memory surfaced that looks wrong or stale — run `vestige-why` before dismissing or forgetting it.
- **Candidate review.** Inspecting a `cand_<ULID>` before approving or rejecting — know what evidence backs it.
- **Forgotten memory audit.** Even soft-deleted memories retain their provenance; inspectable for compliance or review.
- **Source verification.** Need to know whether a memory was typed manually, promoted from a candidate, or captured from an agent session.

## How to invoke

```bash
vestige why <mem_or_cand_id> --json
```

- **`--depth full`**: inline source snippet content in text output (default: summary lines only).
- Accepts `mem_<ULID>` (durable memory) or `cand_<ULID>` (candidate, pre-approval).

```bash
vestige why mem_01JWXXXXXXXXXXXXXXXXXX              # text output
vestige why mem_01JWXXXXXXXXXXXXXXXXXX --json       # JSON output
vestige why mem_01JWXXXXXXXXXXXXXXXXXX --depth full # inline source snippets
vestige why cand_01JVXXXXXXXXXXXXXXXXXX             # pre-approval candidate
```

## After invocation

```json
{
  "memory_id": "mem_01JWXXXXXXXXXXXXXXXXXX",
  "candidate_id": null,
  "subject_type": "decision",
  "status": "active",
  "provenance": {
    "events": [
      { "event_id": "evt_01JWXXXXXXXXXXXXXXXXXX", "type": "memory.recorded", "at": "2026-05-08T11:24:03Z" }
    ],
    "candidate": {
      "candidate_id": "cand_01JVXXXXXXXXXXXXXXXXXX",
      "events": [
        { "event_id": "evt_01JVXXXXXXXXXXXXXXXXXX", "type": "candidate.proposed", "at": "2026-05-08T11:23:47Z" },
        { "event_id": "evt_01JWXXXXXXXXXXXXXXXXXX", "type": "candidate.approved",  "at": "2026-05-08T11:24:03Z" }
      ]
    },
    "sources": [
      { "id": "src_01JW…", "kind": "candidate",     "source_ref": "cand_01JVXX…" },
      { "id": "src_01JV…", "kind": "agent_session", "source_ref": "current", "content": "…", "truncated": false }
    ]
  },
  "status_history": [
    { "at": "2026-05-08T11:24:03Z", "event_type": "memory.recorded" }
  ]
}
```

Cite the `memory_id` or `candidate_id` handle when referencing findings. Use `vestige-sources <id>` for a raw tabular view of source receipts without the event narrative.
