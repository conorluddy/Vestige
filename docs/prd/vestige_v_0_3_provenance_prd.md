# Vestige V0.3 PRD — Provenance and Receipts

## 1. Product Summary

Vestige V0.3 introduces the **Provenance and Receipts** layer: every memory should be answerable to "where did this come from?" and every recall should be answerable to "what did the agent ask, and what did it get?".

V0 proved explicit project memory. V0.1 added embeddings and hybrid recall. V0.2 added the assimilation inbox so capture became reviewable. V0.3 makes the system **inspectable end-to-end**:

```text
memory ──► candidate? ──► raw event ──► source snippet
   ▲                                          │
   └──── recalled by ◄──── query trace ◄──────┘
```

The goal is not to add more memories. The goal is to make the memories Vestige already has **traceable, replayable, and trustworthy** — for humans auditing their store, and for agents reasoning about whether to trust a recalled memory.

## 2. Product Thesis

A memory you cannot explain is a memory you cannot trust.

If an agent surfaces a "decision" memory in a recall, the agent (and the human reviewing) should be able to ask:

```text
Why does this memory exist?
What evidence backs it?
Was it written directly, or promoted from a candidate?
Which session captured it?
Has anything changed about it since?
```

And, symmetrically, when something feels wrong with recall:

```text
What did the agent search for an hour ago?
Which memories did it pull back?
What were the scores?
Was hybrid mode used, or did we fall back to lexical?
```

V0.3 makes both questions answerable from the local store, with no additional capture required at write time. The data has been accumulating in `memory_events`, `memory_sources`, and the candidate audit trail since V0 — V0.3 surfaces it.

This keeps Vestige aligned with its core principles:

- project-scoped memory
- human inspectability
- progressive disclosure
- source-of-truth separation
- agent-safe defaults

## 3. Goals

V0.3 should enable Vestige to:

1. Trace any memory back to its originating event(s), candidate (if any), and source evidence.
2. Surface a single human-readable "why does this memory exist?" walk.
3. List the raw source rows for any memory or candidate.
4. Log every recall query (search / expand / context) to a durable, project-scoped trace table.
5. Replay a query trace by ID — same query, same mode, same parameters.
6. Tag traces with caller (`cli` | `mcp`) so agent-driven recall sessions are inspectable.
7. Expose provenance through MCP without expanding the tool count unnecessarily (extend `vestige_expand` rather than adding three new tools).
8. Strengthen the source model so future "why" walks are cheap (indexed `memory_id` on the journal, typed `source_kind` enum on sources).
9. Cap trace storage with a configurable, predictable retention policy.
10. Keep all flows scriptable via CLI and accessible through MCP.

## 4. Non-Goals

V0.3 should not include:

- Background daemon or trace shipping.
- Cross-project provenance (federated "why" across repos) — waits for V0.9.
- LLM-generated provenance narratives. The "why" walk is templated, not synthesised.
- Automatic dedup based on source overlap.
- Source diffing (was this snippet edited since capture?).
- Multi-source-at-record-time (`record_memory` keeps its single-source signature; multi-source via post-hoc `add_memory_source` is unchanged).
- Location fields on file/commit sources (`line_range`, `commit_sha`, `byte_range`) — deferred to V0.4.
- Trace replay across embedding model versions (replay is best-effort; if the provider has changed, surface the mismatch and run with current provider).
- GUI dashboard.
- Hard delete of traces. Eviction is FIFO by configured cap, not user-driven.
- Editing or annotating an existing memory's provenance. Provenance is derived from the journal — to "fix" provenance, you re-record.

## 5. Target User

The primary user remains the solo developer or agent-heavy builder using Claude Code, Codex, Cursor, local agents, or custom MCP workers across one or more repos.

The specific V0.3 user problems:

> "My agent surfaced a decision I don't remember making. I need to know whether that was a real capture or a hallucinated dedup."

> "Recall feels worse than yesterday. I want to see what the last 20 queries actually returned and what changed."

> "I'm reviewing the memory store before a long session. I want a one-line provenance summary for every durable memory."

V0.3 gives that user a typed receipt for every memory and every recall — readable from the CLI, inspectable from MCP, and grounded in data that's already being captured.

## 6. Core Concepts

### 6.1 Provenance Walk

A provenance walk is a deterministic traversal from a memory back to its originating evidence:

```text
mem_<ULID>
  ├── memory_representations           (one_liner / summary / compressed / full)
  ├── memory_sources                   (zero or more rows; typed by source_kind)
  ├── memory_events (status journal)   (memory.recorded → forgotten → restored, etc.)
  └── candidate?                       (if promoted from cand_<ULID>)
        ├── candidate_sources
        └── memory_events              (candidate.proposed → approved)
```

The walk answers:

```text
What happened?
Where did this candidate come from?
What evidence supports it?
What status transitions has it gone through?
```

### 6.2 Source Receipt

A source receipt is the typed evidence row attached to a memory or candidate. V0.3 introduces a typed `source_kind`:

```text
file
commit
url
agent_session
mcp_call
candidate            ← reverse-provenance row written on candidate approval (V0.2)
manual               ← `vestige *.add` with no --source
trace                ← memory captured during a recall session (forward-link to query_events)
```

Source receipts answer:

```text
Where, exactly, did this come from?
Can I open the file/URL/session it cites?
Was the snippet truncated at the 2 KiB cap?
```

### 6.3 Query Trace

A query trace is an append-only record of a single recall call. Every search / expand / context invocation produces one trace row, regardless of caller (CLI or MCP).

Query traces answer:

```text
What did the agent ask?
Which mode (lexical / semantic / hybrid) was used?
What memory IDs came back, in what order, with what scores?
How long did it take?
Was the requested mode honoured, or did we fall back?
```

Traces are project-scoped and FIFO-evicted by a configurable cap.

### 6.4 Replay

Replay re-runs a stored trace's query against the current store and current provider. The result is **the same query, executed now** — not a snapshot of the historical result. Differences between the original returned IDs and the replay are surfaced explicitly.

Replay answers:

```text
If I ran that same query now, would I get the same memories?
Has the corpus drifted?
Has the provider drifted?
```

## 7. User Experience

### 7.1 Why Path

A surfaced memory feels suspicious. The user runs:

```bash
vestige why mem_01H...
```

Output:

```text
mem_01H...  decision  importance 0.7
  Title: Use dual skill targets for cross-agent support
  Status: active  (since 2026-04-12)

Provenance walk:
  ◇ Recorded   2026-04-12 11:24:03  (evt_01H...)
  ◆ Promoted from candidate cand_01H...
  ◇ Proposed   2026-04-12 11:23:47  (evt_01J...)  source: agent_session

Sources (2):
  ─ candidate    ref=cand_01H...      reverse-provenance link
  ─ agent_session ref=current          "We decided to install bundled skills…"

Status history:
  2026-04-12 11:24  recorded
  (no further transitions)
```

The walk is templated — no LLM. Every line maps 1:1 to a row in `memory_events`, `memory_sources`, or `candidate_memories`.

### 7.2 Sources Path

The user wants the raw evidence, not the narrative:

```bash
vestige sources mem_01H...
```

Output:

```text
mem_01H...  2 sources

src_01H...  candidate       cand_01H...
src_01J...  agent_session   current
            "We decided to install bundled skills to both .claude/skills
             and .agents/skills so they work across Codex and Claude Code."
            (truncated: false; 1.2 KiB / 2 KiB)
```

`vestige sources` also accepts `cand_<ULID>` to inspect candidate sources.

### 7.3 Trace Path

Recall feels off. The user lists the recent traces:

```bash
vestige trace
```

Output:

```text
Recent query traces (last 10):

trace_01H...  2026-05-08 14:02:11  search   hybrid    "skill install dual target"   3 results  47ms  caller=mcp
trace_01J...  2026-05-08 14:02:08  expand   —         mem_01H... (depth=summary)    1 result   2ms   caller=mcp
trace_01K...  2026-05-08 13:58:42  context  —         (project bootstrap)           12 items   18ms  caller=cli
…
```

Inspecting a single trace:

```bash
vestige trace trace_01H...
```

Output:

```text
trace_01H...   search · hybrid    caller=mcp
Time:          2026-05-08 14:02:11 (47ms)
Query:         "skill install dual target"
Mode requested: hybrid     resolved: hybrid
Provider:      fastembed   model: BAAI/bge-small-en-v1.5
Limit: 10      Type filter: —

Results (3):
  1. mem_01H...  0.91   "Use dual skill targets for cross-agent support"
  2. mem_01J...  0.74   "Skills install path layout"
  3. mem_01K...  0.62   "agentskills.io compatibility"

Score parts: lexical+vector merged via reciprocal rank fusion.
```

### 7.4 Replay Path

```bash
vestige trace replay trace_01H...
```

Output:

```text
Replaying trace_01H...

Original (2026-05-08 14:02:11):
  1. mem_01H...  0.91
  2. mem_01J...  0.74
  3. mem_01K...  0.62

Now (2026-05-08 14:30:02):
  1. mem_01H...  0.91   (unchanged)
  2. mem_01J...  0.78   (score +0.04)
  3. mem_01M...  0.65   (new)
  ─ mem_01K...   dropped from top results

Provider: fastembed BAAI/bge-small-en-v1.5  (matches original)
Corpus drift: +1 memory since original.
```

Replay never mutates state. It re-runs and diffs.

## 8. Data Model

### 8.1 Trace IDs

Trace IDs use a distinct prefix:

```text
trace_<ULID>
```

Memory, project, event, candidate, embedding ID conventions (`mem_`, `proj_`, `evt_`, `cand_`, `emb_`) are unchanged.

### 8.2 Query Events Table

V0.3 introduces a dedicated `query_events` table. It is intentionally separate from `memory_events`: cardinality is much higher (1000s/day vs 10s), retention pressure is different, and it is read-only audit data, not part of the source-of-truth chain.

```sql
CREATE TABLE query_events (
    id              TEXT PRIMARY KEY,           -- trace_<ULID>
    project_id      TEXT NOT NULL,
    kind            TEXT NOT NULL,              -- search | expand | context
    mode_requested  TEXT,                       -- lexical | semantic | hybrid (search only)
    mode_resolved   TEXT,                       -- actual mode after fallback
    query_text      TEXT,                       -- ≤ 1 KiB, truncated at UTF-8 boundary
    params_json     TEXT,                       -- limit, type filter, depth, etc.
    caller          TEXT NOT NULL,              -- cli | mcp
    provider        TEXT,                       -- e.g. "fastembed"
    provider_model  TEXT,                       -- e.g. "BAAI/bge-small-en-v1.5"
    result_ids_json TEXT,                       -- ordered JSON array of mem_<ULID>
    result_scores_json TEXT,                    -- parallel array of scores; null for non-search
    result_count    INTEGER NOT NULL DEFAULT 0,
    latency_ms      INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE INDEX idx_query_events_project_created
    ON query_events (project_id, created_at DESC);
CREATE INDEX idx_query_events_kind
    ON query_events (project_id, kind, created_at DESC);
```

Eviction: when row count for a `project_id` exceeds the configured cap, drop oldest rows. Run opportunistically at write time (cheap COUNT then DELETE LIMIT N) — no daemon.

### 8.3 Memory Events: Indexed Memory ID

`memory_events.payload_json` already carries `memory_id` for memory-related events, but lookups require JSON extraction. V0.3 adds an indexed nullable column:

```sql
ALTER TABLE memory_events ADD COLUMN memory_id TEXT;
CREATE INDEX idx_memory_events_memory_id
    ON memory_events (memory_id, created_at DESC);
```

Migration backfills `memory_id` from `payload_json` for existing rows where the payload contains it. Subsequent writes in `record.rs`, `lifecycle.rs`, etc. populate the column directly.

The column is nullable because some events are project-scoped without a memory (e.g. future `query.executed` if we ever choose to mirror traces into the journal — not planned for V0.3).

### 8.4 Source Kind Enum

`memory_sources.source_type` and `candidate_sources.source_type` are currently free-form `TEXT`. V0.3 narrows them via application-layer validation (no SQL CHECK — keeps migrations forward-compatible) to:

```text
file
commit
url
agent_session
mcp_call
candidate
manual
trace
```

Existing rows are not rewritten. The validator accepts any string for read-back compatibility but rejects unknown values on write. A new `SourceKind` enum lives in `vestige-core::types`.

The `trace` kind is forward-looking: when a future skill captures a memory during a recall session, it will record the originating `trace_<ULID>` as the source ref.

### 8.5 Memory Provenance View

For cheap "why" queries, V0.3 adds a view that pre-joins the journal:

```sql
CREATE VIEW memory_provenance AS
SELECT
    m.id              AS memory_id,
    m.project_id      AS project_id,
    e.id              AS event_id,
    e.event_type      AS event_type,
    e.payload_json    AS payload_json,
    e.created_at      AS event_at
FROM memories m
LEFT JOIN memory_events e
       ON e.memory_id = m.id
ORDER BY e.created_at;
```

The view is a convenience — `vestige why` reads it directly rather than re-deriving the join. Views are migration-friendly (drop+recreate, no data loss).

## 9. CLI Requirements

### 9.1 `vestige why`

```bash
vestige why mem_01H...
vestige why mem_01H... --json
vestige why cand_01H...                     # candidates accepted
vestige why mem_01H... --depth full         # include source contents inline
```

Default output: templated provenance walk (see §7.1). `--depth full` inlines source snippets.

### 9.2 `vestige sources`

```bash
vestige sources mem_01H...
vestige sources mem_01H... --json
vestige sources cand_01H...
vestige sources mem_01H... --kind agent_session   # filter
```

Default output: tabular source receipts (see §7.2).

### 9.3 `vestige trace`

```bash
vestige trace                               # list recent traces (default 10)
vestige trace --limit 50
vestige trace --kind search
vestige trace --caller mcp
vestige trace --since 2026-05-08
vestige trace trace_01H...                  # show one trace
vestige trace trace_01H... --json
vestige trace replay trace_01H...           # re-run and diff
vestige trace replay trace_01H... --json
```

Default output: compact list (see §7.3). `--json` for scripting.

### 9.4 No mutation commands

V0.3 adds no write paths. `why`, `sources`, and `trace list/show` are read-only. `trace replay` re-runs through `vestige-engine` and writes a new trace row of its own (clearly tagged as a replay in `params_json`), but does not mutate the original trace or any memory.

## 10. MCP Requirements

### 10.1 MCP Philosophy

V0.3 minimises new tool surface. The provenance question is *intent: drill into this memory* — that's already what `vestige_expand` answers. The trace question is genuinely new intent.

### 10.2 Extended Tool: `vestige_expand`

`vestige_expand` already accepts a memory ID and returns progressive depths. V0.3 adds:

```json
{
  "id": "mem_01H...",
  "depth": "provenance"
}
```

When `depth` is `"provenance"`, the response includes the structured provenance walk:

```json
{
  "memory_id": "mem_01H...",
  "type": "decision",
  "status": "active",
  "provenance": {
    "events": [
      { "event_id": "evt_01H...", "type": "memory.recorded", "at": "..." }
    ],
    "candidate": {
      "candidate_id": "cand_01H...",
      "events": [
        { "event_id": "evt_01J...", "type": "candidate.proposed", "at": "..." },
        { "event_id": "evt_01K...", "type": "candidate.approved", "at": "..." }
      ]
    },
    "sources": [
      { "source_id": "src_01H...", "kind": "candidate", "ref": "cand_01H..." },
      { "source_id": "src_01J...", "kind": "agent_session", "ref": "current",
        "content_preview": "We decided to…", "truncated": false }
    ]
  }
}
```

`depth=provenance` is additive to existing depths (`one_liner` / `summary` / `compressed` / `full`). It does not replace any.

### 10.3 New Tool: `vestige_trace`

Purpose: replay or inspect query traces from an agent.

Input shape:

```json
{
  "action": "list",                    // list | show | replay
  "trace_id": "trace_01H...",          // required for show/replay
  "limit": 10,                         // list only
  "kind": "search",                    // list filter
  "since": "2026-05-08T00:00:00Z"      // list filter
}
```

Output (list):

```json
{
  "traces": [
    {
      "trace_id": "trace_01H...",
      "kind": "search",
      "mode": "hybrid",
      "query": "skill install dual target",
      "result_count": 3,
      "latency_ms": 47,
      "caller": "mcp",
      "created_at": "..."
    }
  ]
}
```

Output (replay):

```json
{
  "trace_id": "trace_01H...",
  "original": { "result_ids": ["mem_01H...", ...], "scores": [0.91, ...] },
  "current":  { "result_ids": ["mem_01H...", ...], "scores": [0.91, ...] },
  "diff": {
    "added":   ["mem_01M..."],
    "removed": ["mem_01K..."],
    "score_changes": [{ "id": "mem_01J...", "delta": 0.04 }]
  },
  "provider_match": true,
  "corpus_drift": 1
}
```

### 10.4 Tracing the MCP Surface

Every MCP call to `vestige_search`, `vestige_expand`, and `vestige_get_project_context` writes one `query_events` row with `caller="mcp"`. Tracing happens in `vestige-engine` so both CLI and MCP get it for free with no per-call code in the MCP tool layer.

`vestige_record_observation`, `vestige_record_decision`, `vestige_propose_candidate`, `vestige_list_candidates`, `vestige_get_candidate` are **not** traced — those are mutations or candidate-inbox operations and are already covered by `memory_events`.

`vestige_bootstrap` is treated as a `context` kind trace.

### 10.5 Tracing the CLI Surface

CLI commands `vestige search`, `vestige expand`, and `vestige context` write `query_events` rows with `caller="cli"`. CLI mutation commands (`record`, `forget`, `restore`, `approve`, `reject`) are not traced; they continue using `memory_events`.

## 11. Search and Recall Behaviour

### 11.1 Trace Visibility

Query traces never appear in normal recall. They are not memories. `vestige search`, `vestige recall`, `vestige context` ignore the `query_events` table entirely.

### 11.2 Trace Self-Reference

`vestige trace replay` itself produces a trace row. The replayed trace's `params_json` includes `replay_of: trace_<ULID>` so the chain is inspectable. Replays of replays are allowed but unusual.

### 11.3 Provenance and Soft-Delete

`vestige why` and `vestige sources` work for `status='deleted'` memories — provenance must remain inspectable for forgotten memories so users can audit what was forgotten. The walk surfaces the `memory.forgotten` event and `deleted_at`.

## 12. Provenance Requirements

V0.3 hardens the chain established by V0.1 and V0.2:

```text
mem ──► memory_events  (status timeline)
mem ──► memory_sources (typed evidence rows)
mem ──► candidate? ──► candidate_sources, candidate events
```

For every approved memory, V0.3 must surface:

- candidate ID (from the reverse-provenance `memory_sources` row written in V0.2)
- source event IDs from both the memory and candidate journals
- original source refs and content (subject to the 2 KiB cap)
- approval timestamp
- whether the candidate was edited at approval time

For every directly-recorded memory (no candidate):

- `memory.recorded` event with full payload
- one or more `memory_sources` rows if `--source` was provided
- `manual` source kind if recorded with no source

## 13. JSON Output Requirements

All new CLI commands support `--json`. Shapes mirror the MCP responses in §10 wherever possible to keep one shape across surfaces.

### 13.1 `vestige why --json`

```json
{
  "memory_id": "mem_01H...",
  "type": "decision",
  "status": "active",
  "provenance": { /* same shape as MCP vestige_expand depth=provenance */ },
  "status_history": [
    { "event_id": "evt_01H...", "type": "memory.recorded", "at": "..." }
  ]
}
```

### 13.2 `vestige sources --json`

```json
{
  "owner_id": "mem_01H...",
  "owner_kind": "memory",
  "sources": [
    { "id": "src_01H...", "kind": "candidate", "ref": "cand_01H...",
      "content": null, "truncated": false }
  ]
}
```

### 13.3 `vestige trace --json` (list and show)

Mirrors §10.3.

## 14. Configuration

```toml
[traces]
enabled = true
max_per_project = 10000        # FIFO eviction when exceeded
truncate_query_text_bytes = 1024
trace_caller_cli = true
trace_caller_mcp = true
```

V0.3 ships these as defaults. `enabled = false` disables trace writes entirely (read paths still work for older traces). `trace_caller_cli` / `trace_caller_mcp` allow disabling tracing per surface without disabling the feature.

## 15. Implementation Plan

### Milestone 1 — Schema and Source Model

Deliverables:

- Migration `0005_provenance.sql`:
  - `query_events` table + indexes
  - `memory_events.memory_id` column + index
  - `memory_provenance` view
  - Backfill of `memory_events.memory_id` from `payload_json`
- `SourceKind` enum in `vestige-core::types` with validator
- `TraceId` newtype (`trace_<ULID>`)
- Update `record_memory` and friends to populate `memory_events.memory_id` directly

Acceptance criteria:

- Migration applies cleanly to V0.2 DBs.
- Backfill populates `memory_id` for at least all `memory.recorded`, `memory.forgotten`, `memory.restored`, `candidate.approved` events.
- `SourceKind` parser rejects unknown kinds on write, accepts on read.
- `TraceId::parse` rejects wrong prefix.

### Milestone 2 — Engine Tracing Hook

Deliverables:

- `vestige-engine`: trace-write helper that wraps `search_lexical` / `search_semantic` / `search_hybrid` / `expand` / `get_project_context`.
- Caller passed in via parameter (`Caller::Cli` | `Caller::Mcp`).
- FIFO eviction at write time when over cap.
- Provider/model recorded for search traces.

Acceptance criteria:

- Every search/expand/context call from CLI writes one `query_events` row.
- Every search/expand/context call from MCP writes one `query_events` row.
- Mutations write zero `query_events` rows.
- Cap enforcement deletes oldest rows for the project once exceeded.

### Milestone 3 — `vestige why` and `vestige sources`

Deliverables:

```bash
vestige why <mem_or_cand_id>
vestige sources <mem_or_cand_id> [--kind ...]
```

Acceptance criteria:

- `why` walk for a directly-recorded memory shows the recorded event and any sources.
- `why` walk for a candidate-promoted memory shows both journals and the reverse-provenance link.
- `why` walks for a soft-deleted memory show the forgotten event.
- `sources` filters by `--kind` correctly.
- `--json` output validates against the documented shape.

### Milestone 4 — `vestige trace` (list + show)

Deliverables:

```bash
vestige trace
vestige trace <trace_id>
```

Acceptance criteria:

- List shows recent traces with kind / mode / query / count / caller.
- Show renders parameters, results, scores, and resolution metadata.
- Type and caller filters work.
- `--json` output validates.

### Milestone 5 — `vestige trace replay`

Deliverables:

```bash
vestige trace replay <trace_id>
```

Acceptance criteria:

- Replay re-runs the trace through `vestige-engine` with current store + provider.
- Diff lists added / removed / score_changes correctly.
- Provider mismatch surfaces explicitly (not silently re-runs).
- Replay writes its own trace row tagged with `replay_of` in `params_json`.

### Milestone 6 — MCP Provenance + Trace Tools

Deliverables:

- `vestige_expand` accepts `depth = "provenance"` and returns the structured walk.
- New `vestige_trace` tool with `action = list | show | replay`.
- Engine-layer tracing automatically tags MCP calls as `caller=mcp`.

Acceptance criteria:

- MCP smoke tests for `vestige_expand` at `depth=provenance` (memory + candidate-promoted memory + soft-deleted memory).
- MCP smoke tests for `vestige_trace` list / show / replay.
- Tool count: `vestige_expand` extended, `vestige_trace` added. No other tools touched.
- `vestige_search` calls produce `caller=mcp` traces visible from `vestige trace`.

### Milestone 7 — Configuration and Eviction

Deliverables:

- `[traces]` config block in `vestige-config`.
- `max_per_project` enforced.
- `enabled = false` disables trace writes (reads continue).
- Per-surface toggles honoured.

Acceptance criteria:

- Setting `max_per_project = 5` and writing 10 traces leaves exactly 5 most recent.
- Setting `enabled = false` produces zero new rows; existing rows still readable.
- Disabling `trace_caller_mcp` while leaving `trace_caller_cli = true` traces only CLI calls.

### Milestone 8 — Docs and Demo

Deliverables:

- README V0.3 section.
- PRD linked from main README and CLAUDE.md.
- Landing page (`docs/src/data.js`) updated: V0.3 row → `done`; V0.4 → `now`.
- `docs/v0.3.md` walkthrough mirroring `docs/v0.2.md`.

Acceptance criteria:

- A new user can read README → understand `why` / `sources` / `trace` → run them against a fresh repo.
- Landing roadmap is accurate.

## 16. Testing Requirements

### Unit Tests

- `SourceKind` parser: all valid values + reject on unknown for write.
- `TraceId::parse` rejects wrong prefix.
- Source-snippet truncation invariant (existing) re-asserted for trace `query_text`.
- Backfill SQL extracts `memory_id` from payload variants correctly.

### Store Integration Tests

- Migration applies cleanly to a synthetic V0.2 DB and backfills `memory_id`.
- `query_events` insert + index lookup.
- FIFO eviction: insert N+1 over cap, oldest dropped.
- Project-scoped trace isolation: project A traces invisible from project B.
- `memory_provenance` view returns expected joins for directly-recorded and candidate-promoted memories.
- Soft-deleted memory still has its events queryable via `memory_id` index.

### Engine Tests

- Each of `search_lexical` / `search_semantic` / `search_hybrid` / `expand` / `context` writes exactly one trace row per call, with correct `kind` and `mode`.
- Mutations write zero trace rows.
- Replay diffing: identical corpus → empty diff; added memory → `added` populated; forgotten memory → `removed` populated.
- Provider mismatch on replay surfaces in the response (`provider_match: false`).

### CLI Smoke Tests

- `record → why` shows the recorded event + manual source.
- `record --source ... → sources` lists the source row.
- `candidate add → approve → why` shows both journals + reverse-provenance link.
- `forget → why` shows the forgotten event and current `deleted` status.
- `search → trace` lists the search; `trace <id>` shows the result IDs; `trace replay <id>` re-runs.
- `--json` shapes validate.

### MCP Tests

- `vestige_expand` at `depth=provenance` returns the structured walk for memory and candidate-promoted memory.
- `vestige_trace` list/show/replay round-trip.
- An MCP `vestige_search` call appears in `vestige trace` with `caller=mcp`.

### Regression Tests

- Normal `vestige search` does not return trace rows.
- Normal `vestige context` does not include trace rows.
- V0.2 candidate flows unaffected: approval still writes the reverse-provenance source row, candidate FTS dedup still works.
- Soft-delete still excludes from search (FTS trigger sync unchanged).
- Restore still re-indexes (inverse trigger unchanged).

## 17. Acceptance Criteria

V0.3 is complete when:

- Migration `0005_provenance.sql` applies cleanly to V0.2 DBs and backfills `memory_events.memory_id`.
- `vestige why <id>` returns a templated provenance walk for any memory or candidate, including soft-deleted ones.
- `vestige sources <id>` lists typed source receipts and supports `--kind` filtering.
- `vestige trace` lists recent recall traces; `vestige trace <id>` shows full detail.
- `vestige trace replay <id>` re-runs the query and diffs against the original.
- Every `search` / `expand` / `context` call from CLI or MCP writes one `query_events` row, tagged with caller.
- FIFO eviction enforces `max_per_project` (default 10000).
- `vestige_expand` accepts `depth=provenance` and returns the structured walk.
- `vestige_trace` MCP tool supports `list` / `show` / `replay`.
- `SourceKind` is a typed enum at the application layer; rejection on write of unknown kinds.
- All new flows support `--json`.
- `[traces]` config block implemented; `enabled=false` and per-surface toggles honoured.
- Tests cover provenance walks, trace lifecycle, eviction, replay diffing, and recall isolation.
- README, PRD, and landing-page roadmap updated.

## 18. Open Questions

1. Should `memory_provenance` be a SQL view or a Rust function? View is cheaper to migrate but harder to evolve; function gives richer error handling. **Lean: view for V0.3, function in V0.4 if it gets complex.**
2. Should replay re-write the original trace's `result_ids_json` if the agent wants to "accept the new results as canonical"? **Lean: no — traces are append-only audit. Acceptance is implicit by writing a new trace.**
3. Should `vestige why` ever inline LLM-generated summaries of long source content? **Lean: no for V0.3. Templated only.**
4. Should the trace cap be global (across all projects) or per-project? **Lean: per-project (matches scope boundary). Global cap deferred.**
5. How should `vestige trace` represent context calls — full bootstrap pack JSON in `params_json`, or just the request shape and a count? **Lean: just the request shape + count. Full pack is too large.**
6. Should we trace `vestige expand depth=provenance` as a `provenance` kind, or fold it under `expand`? **Lean: under `expand` with `depth` in `params_json`.**
7. Should `manual` and `trace` source kinds appear in the V0.3 enum even if no current writer produces `trace`? **Lean: yes — reserve the slot now to avoid an enum-bump migration later.**
8. Should `vestige trace replay` require confirmation if the corpus has drifted significantly (e.g. >20% memory delta)? **Lean: no for V0.3 — replay is read-only.**
9. Should provider/model be stored on every trace, or only when mode is `semantic` / `hybrid`? **Lean: only when relevant — null for `lexical` and non-search kinds.**
10. Should we expose `trace.replayed` events back into `memory_events`? **Lean: no — keep journals separate; replays live entirely in `query_events`.**

## 19. Recommended First Slice

Build this first:

```bash
# 1. Schema only — migration + backfill, no CLI yet
cargo run -p vestige -- status

# 2. Read-only "why" walk against existing data
vestige why mem_01H...
vestige sources mem_01H...

# 3. Engine trace-write hook for search only
vestige search "skills" --mode hybrid
vestige trace

# 4. Show one trace
vestige trace trace_01H...
```

No replay. No MCP. No expand-trace. No context-trace.

Once that proves the data flow end-to-end, layer on:

- Replay
- Expand and context tracing
- MCP `depth=provenance` and `vestige_trace`
- Configuration + eviction

That keeps the first PR small, exercises the schema and the templated walk against real V0.2 data, and lets us catch backfill edge cases before we widen the trace surface.
