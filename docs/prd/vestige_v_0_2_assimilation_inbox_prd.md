# Vestige V0.2 PRD — Assimilation Inbox

## 1. Product Summary

Vestige V0.2 introduces the **Assimilation Inbox**: a review layer between automatically detected project context and durable project memory.

V0 proved explicit project memory. V0.1 added embeddings and hybrid recall. V0.2 should make automatic capture safer by changing the default flow from:

```text
agent notices something → memory is written
```

to:

```text
agent notices something → candidate is proposed → user/agent reviews → memory is approved or rejected
```

The goal is not to ingest everything. The goal is to let agents notice useful context freely while keeping long-lived project memory inspectable, reviewable, and controlled.

## 2. Product Thesis

Automatic memory is only useful if it remains trustworthy.

If agents write directly into durable memory, the store can decay into noise: duplicated decisions, half-true notes, stale TODOs, speculative observations, and context that future agents over-trust.

The Assimilation Inbox creates a safety membrane:

```text
Raw evidence is captured.
Candidate memories are proposed.
Only reviewed candidates become durable memories.
Rejected candidates remain as audit/history but do not pollute recall.
```

This keeps Vestige aligned with its core principles:

- project-scoped memory
- human inspectability
- progressive disclosure
- source-of-truth separation
- agent-safe defaults

## 3. Goals

V0.2 should enable Vestige to:

1. Capture raw project/session events without immediately promoting them to memory.
2. Generate candidate memories from those events.
3. List pending candidates in a compact review inbox.
4. Show candidate detail, source evidence, proposed type, confidence, and possible duplicates.
5. Approve a candidate into a normal `mem_<ULID>` memory.
6. Reject a candidate with a reason.
7. Prevent candidates from appearing in normal recall until approved.
8. Update bundled agent skills so proactive auto-memorise writes candidates instead of durable memories.
9. Preserve traceability from approved memory back to candidate and raw event.
10. Keep all flows scriptable via CLI and eventually accessible through MCP.

## 4. Non-Goals

V0.2 should not include:

- Background daemon.
- Continuous passive ingestion.
- GitHub/Slack/email/browser integrations.
- Cross-project candidate inbox.
- GUI dashboard.
- Full contradiction detection.
- LLM-heavy summarisation pipelines as a hard dependency.
- Automatic candidate approval by default.
- Hard delete of rejected candidates.
- Team collaboration or sync.
- Global preference assimilation.

## 5. Target User

The primary user remains the solo developer or agent-heavy builder using Claude Code, Codex, Cursor, local agents, or custom MCP workers across one or more repos.

The specific V0.2 user problem:

> “My agent keeps discovering useful project context, but I do not want every observation immediately stored as trusted memory.”

V0.2 gives that user a review queue where useful findings can accumulate without corrupting durable recall.

## 6. Core Concepts

### 6.1 Raw Event

A raw event is durable evidence that something happened.

Examples:

```text
agent_session_message
agent_tool_result
manual_cli_capture
mcp_record_call
debugging_conclusion
planning_decision
file_reference
commit_reference
```

Raw events are append-only and are not returned by normal recall.

They answer:

```text
What happened?
Where did this candidate come from?
Can I inspect the original evidence?
```

### 6.2 Candidate Memory

A candidate memory is a proposed interpretation of one or more raw events.

It has a proposed memory type, title, body, confidence, importance, and optional duplicate hints.

Candidate memories are not trusted memory. They are pending review.

They answer:

```text
What does Vestige think is worth remembering?
Why was this proposed?
What source evidence supports it?
Is it similar to something already stored?
```

### 6.3 Promoted Memory

A promoted memory is a normal Vestige memory created from an approved candidate.

It receives a `mem_<ULID>` ID, standard representations, FTS indexing, optional embedding, and appears in search/recall/context like any other memory.

### 6.4 Rejected Candidate

A rejected candidate remains stored as audit history, but never enters normal recall.

Rejection reasons are used later to tune auto-memorise, deduplication, and future Directives.

## 7. User Experience

### 7.1 Happy Path

A coding session produces a durable decision:

```text
“We’ll use .agents/skills/ alongside .claude/skills/ so skills work across Codex and Claude Code.”
```

An agent skill detects the moment and writes a candidate:

```text
Candidate captured: cand_01H... decision
```

The user later runs:

```bash
vestige inbox
```

Output:

```text
Pending candidates: 1

cand_01H...  decision  0.82  Use dual skill targets for cross-agent support
             Source: agent_session
             Similar: none
```

The user inspects it:

```bash
vestige inbox show cand_01H...
```

Then approves:

```bash
vestige approve cand_01H...
```

Vestige creates:

```text
Approved cand_01H... → mem_01H...
```

The memory now appears in:

```bash
vestige recall "why do we install skills twice?"
```

### 7.2 Rejection Path

A noisy candidate appears:

```text
cand_02H... note 0.44 "Maybe add Electron GUI soon"
```

The user rejects:

```bash
vestige reject cand_02H... --reason "not durable"
```

It remains in candidate history but does not appear in recall.

### 7.3 Duplicate Path

A candidate overlaps with an existing memory:

```text
cand_03H... decision 0.79 "MCP should stay a thin adapter"
             Similar: mem_01H... "MCP as thin adapter"
```

The user can reject as duplicate:

```bash
vestige reject cand_03H... --reason duplicate --duplicate-of mem_01H...
```

Or approve anyway if it captures a meaningful refinement.

## 8. Data Model

### 8.1 Candidate IDs

Candidate IDs should use a distinct prefix:

```text
cand_<ULID>
```

Raw events continue to use:

```text
evt_<ULID>
```

Approved memories continue to use:

```text
mem_<ULID>
```

### 8.2 Raw Events Table

V0.2 may extend the existing `memory_events` table or introduce a clearer `raw_events` table.

Preferred direction: keep `memory_events` as the durable append-only journal if it can support this cleanly.

Suggested shape:

```sql
CREATE TABLE raw_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  source_type TEXT NOT NULL,
  source_ref TEXT,
  content TEXT,
  content_hash TEXT,
  metadata_json TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id)
);
```

If reusing `memory_events`, equivalent fields should be encoded in `event_type` and `payload_json`.

### 8.3 Candidate Memories Table

```sql
CREATE TABLE candidate_memories (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  source_event_id TEXT,
  proposed_type TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  title TEXT NOT NULL,
  one_liner TEXT NOT NULL,
  summary TEXT,
  full_body TEXT NOT NULL,
  rationale TEXT,
  confidence REAL NOT NULL DEFAULT 0.5,
  importance REAL NOT NULL DEFAULT 0.5,
  dedup_key TEXT,
  duplicate_of_memory_id TEXT,
  duplicate_of_candidate_id TEXT,
  approved_memory_id TEXT,
  rejection_reason TEXT,
  review_note TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  reviewed_at TEXT,
  FOREIGN KEY (project_id) REFERENCES projects(id),
  FOREIGN KEY (source_event_id) REFERENCES raw_events(id)
);
```

If the project keeps source evidence in `memory_sources`, candidates may also need candidate-specific sources:

```sql
CREATE TABLE candidate_sources (
  id TEXT PRIMARY KEY,
  candidate_id TEXT NOT NULL,
  source_type TEXT NOT NULL,
  source_ref TEXT,
  source_content TEXT,
  truncated INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  FOREIGN KEY (candidate_id) REFERENCES candidate_memories(id)
);
```

### 8.4 Candidate Statuses

V0.2 should support:

```text
pending
approved
rejected
superseded
```

Deferred statuses:

```text
archived
needs_edit
auto_rejected
duplicate
```

`duplicate` may be a rejection reason rather than a status in V0.2.

### 8.5 Candidate Types

Candidates should use the same core memory types as promoted memories:

```text
decision
note
preference
open_question
project_summary
observation
```

For V0.2, `decision`, `note`, `preference`, and `open_question` are enough.

## 9. CLI Requirements

### 9.1 `vestige inbox`

Lists pending candidates.

```bash
vestige inbox
vestige inbox --json
vestige inbox --limit 20
vestige inbox --type decision
vestige inbox --include-rejected
```

Default output should be compact.

Example:

```text
Pending candidates: 3

cand_01H...  decision    0.82  Use dual skill targets for cross-agent support
             Source: agent_session
             Similar: none

cand_02H...  note        0.61  README install section is stale
             Source: README.md
             Similar: none

cand_03H...  question    0.55  Should Directives land before daemon heartbeat ingestion?
             Source: roadmap discussion
             Similar: none
```

### 9.2 `vestige inbox show`

Shows candidate detail.

```bash
vestige inbox show cand_01H...
vestige inbox show cand_01H... --json
```

Output should include:

- ID
- status
- proposed type
- confidence
- importance
- title
- one-liner
- full body
- rationale
- source evidence
- similar memories/candidates
- available actions

### 9.3 `vestige candidate add`

Creates a candidate manually or from an agent skill.

```bash
vestige candidate add \
  --type decision \
  --title "Use dual skill targets" \
  --body "Install bundled skills to both .claude/skills and .agents/skills." \
  --rationale "Supports Claude Code and agentskills.io-compatible agents." \
  --source "agent_session:current"
```

Optional flags:

```bash
--importance 0.7
--confidence 0.8
--source-content "..."
--json
```

This is the simplest V0.2 capture path.

### 9.4 `vestige event record`

Optional but recommended if preserving the two-step event/candidate model explicitly.

```bash
vestige event record \
  --type agent_session \
  --source current \
  --content "We decided to install bundled skills to both .claude and .agents."
```

This returns:

```text
evt_01H...
```

Then:

```bash
vestige candidate add --from-event evt_01H... --type decision ...
```

For V0.2, `candidate add` can create the raw event implicitly when `--source-content` is provided.

### 9.5 `vestige approve`

Approves a candidate into a durable memory.

```bash
vestige approve cand_01H...
vestige approve cand_01H... --json
```

Optional edit flags:

```bash
vestige approve cand_01H... \
  --type decision \
  --title "Dual target skills install" \
  --body "Vestige installs bundled skills to both .claude/skills and .agents/skills by default."
```

Approval should:

1. Validate candidate is pending.
2. Build a normal memory bundle.
3. Create memory representations.
4. Copy or link candidate sources.
5. Index in FTS.
6. Mark candidate as approved.
7. Store `approved_memory_id`.
8. Emit a `candidate.approved` event.

Output:

```text
Approved cand_01H... → mem_01H...
```

### 9.6 `vestige reject`

Rejects a candidate.

```bash
vestige reject cand_01H...
vestige reject cand_01H... --reason duplicate
vestige reject cand_01H... --reason duplicate --duplicate-of mem_01H...
vestige reject cand_01H... --reason "not durable"
```

Allowed initial reasons:

```text
duplicate
wrong
not_durable
too_noisy
stale
other
```

Rejection should:

1. Validate candidate is pending.
2. Mark status rejected.
3. Store reason.
4. Store duplicate link if provided.
5. Emit a `candidate.rejected` event.

## 10. MCP Requirements

### 10.1 MCP Philosophy

MCP should expose candidate creation and review carefully.

Agents should be allowed to propose candidates. They should not approve their own candidates by default.

### 10.2 New MCP Tools

Recommended V0.2 tools:

```text
vestige_record_event
vestige_propose_candidate
vestige_list_candidates
vestige_get_candidate
```

Optional/gated:

```text
vestige_approve_candidate
vestige_reject_candidate
```

### 10.3 `vestige_propose_candidate`

Purpose:

Allow an agent to propose a memory without promoting it.

Input:

```json
{
  "type": "decision",
  "title": "Dual target skills install",
  "body": "Vestige installs bundled skills to both .claude/skills and .agents/skills by default.",
  "rationale": "This supports Claude Code and agentskills.io-compatible agents.",
  "importance": 0.7,
  "confidence": 0.8,
  "source": {
    "type": "agent_session",
    "ref": "current"
  }
}
```

Output:

```json
{
  "candidate_id": "cand_01H...",
  "status": "pending",
  "similar": []
}
```

### 10.4 Candidate Approval Safety

Default config:

```toml
[mcp]
allow_candidate_approval = false
allow_candidate_rejection = false
```

If disabled, MCP approval/rejection tools should return structured non-retryable errors.

Reason:

```text
Candidate proposal is safe.
Candidate promotion changes durable memory.
```

Approval should remain CLI-first until the review policy is proven.

## 11. Agent Skills Changes

### 11.1 Current Behaviour

The current bundled `vestige-auto-memorise` skill captures durable memories when it detects decisions, preferences, questions, gotchas, root causes, or useful notes.

### 11.2 V0.2 Behaviour

In V0.2, proactive auto-memorise should write candidates by default.

Old flow:

```text
dedup → vestige decision add / note add / preference add / question add
```

New flow:

```text
dedup active memories + pending candidates → vestige candidate add
```

Explicit user commands like “remember this” can still choose direct memory creation, but proactive ambient capture should prefer candidates.

### 11.3 Skill Output

The skill should surface a quiet line:

```text
Proposed decision cand_01H...
```

Not:

```text
Recorded decision mem_01H...
```

This wording matters. It makes the trust level clear.

### 11.4 Skill Dedup

Before proposing, the skill should check:

```bash
vestige recall "<keywords>" --type <type> --json --limit 3
vestige inbox --type <type> --json
```

A future `vestige candidate dedup` helper could make this cleaner.

## 12. Search and Recall Behaviour

Candidates must not appear in normal:

```bash
vestige search
vestige recall
vestige context
```

Unless explicitly requested:

```bash
vestige search --include-candidates
```

That flag is optional for V0.2 and can be deferred.

Approved candidates become normal memories and should appear in recall immediately after approval.

Rejected candidates should never appear in default recall.

## 13. Deduplication

V0.2 should include basic deduplication. It does not need perfect semantic clustering.

### 13.1 Dedup Against Existing Memories

When adding a candidate:

1. Search active memories of the same type.
2. Use lexical search by default.
3. Use hybrid search if embeddings exist.
4. Attach similar memory handles to the candidate.

If a near-duplicate is found, the candidate may still be created, but should be marked:

```text
possible_duplicate
```

or store:

```text
duplicate_of_memory_id = mem_...
```

### 13.2 Dedup Against Pending Candidates

Candidates should also dedup against pending candidates to avoid inbox spam.

Minimum V0.2 approach:

- same proposed type
- similar title/body hash
- same source ref
- top lexical/hybrid match

### 13.3 Rejected Candidate Memory

Rejected candidates should inform future dedup.

If a very similar candidate was recently rejected as `not_durable`, future proposals can be marked lower confidence or suppressed later.

For V0.2, just store enough information to support this later.

## 14. Provenance Requirements

Every approved memory created from a candidate should be traceable:

```text
mem → candidate → raw event/source
```

Approval should preserve:

- candidate ID
- source event ID
- original source ref
- source content if stored
- approval timestamp
- whether edits were applied during approval

A future `vestige why mem_...` should be able to use this chain.

## 15. JSON Output Requirements

All new CLI commands should support `--json`.

### 15.1 Inbox JSON

```json
{
  "candidates": [
    {
      "id": "cand_01H...",
      "type": "decision",
      "status": "pending",
      "title": "Dual target skills install",
      "one_liner": "Install bundled skills to both .claude/skills and .agents/skills.",
      "confidence": 0.82,
      "importance": 0.7,
      "similar_memories": [],
      "created_at": "2026-05-02T..."
    }
  ]
}
```

### 15.2 Approval JSON

```json
{
  "candidate_id": "cand_01H...",
  "memory_id": "mem_01H...",
  "status": "approved"
}
```

### 15.3 Rejection JSON

```json
{
  "candidate_id": "cand_01H...",
  "status": "rejected",
  "reason": "duplicate",
  "duplicate_of": "mem_01H..."
}
```

## 16. Configuration

Initial config options can be minimal.

Suggested later config:

```toml
[assimilation]
enabled = true
default_capture = "candidate" # candidate | memory
max_pending = 100
dedup_against_rejected_days = 30

[assimilation.auto_memorise]
decisions = true
preferences = true
questions = true
notes = true
min_confidence = 0.5

[mcp]
allow_candidate_approval = false
allow_candidate_rejection = false
```

For V0.2, most of these can be deferred. The important default is:

```text
auto-memorise proposes candidates, not durable memories.
```

## 17. Implementation Plan

### Milestone 1 — Schema and Core Types

Deliverables:

- `CandidateId` newtype with `cand_` prefix.
- Candidate status enum.
- Candidate memory struct.
- Migration for candidate tables.
- Store methods:
  - create candidate
  - list candidates
  - get candidate
  - approve candidate
  - reject candidate

Acceptance criteria:

- Candidate records can be created and listed.
- Candidate IDs validate correctly.
- Candidates are project-scoped.
- Candidates do not appear in normal memory list/search.

### Milestone 2 — CLI Inbox

Deliverables:

```bash
vestige inbox
vestige inbox show cand_...
vestige candidate add ...
```

Acceptance criteria:

- User can create a candidate manually.
- User can inspect pending candidates.
- `--json` works for scripting.
- Output is compact by default.

### Milestone 3 — Approve/Reject

Deliverables:

```bash
vestige approve cand_...
vestige reject cand_...
```

Acceptance criteria:

- Approving creates a normal memory.
- Approved memory appears in `vestige recall`.
- Rejected candidate does not appear in recall.
- Candidate status updates are recorded.
- Approval links memory back to candidate.

### Milestone 4 — Dedup Hints

Deliverables:

- Similar memory lookup when candidate is created.
- Similar pending candidate lookup.
- Candidate display includes similar handles.
- Duplicate rejection reason.

Acceptance criteria:

- Creating a duplicate candidate surfaces likely duplicate memory/candidate.
- Inbox output shows duplicate hints.
- Rejecting as duplicate stores the duplicate relationship.

### Milestone 5 — Skills Integration

Deliverables:

- Update `vestige-auto-memorise` to propose candidates.
- Update capture skills where appropriate.
- Update bundled skill docs/evals.
- Update `vestige skills list/install` bundle.

Acceptance criteria:

- Proactive auto-memorise does not directly create durable memory.
- Skill output says “Proposed … cand_...” not “Recorded … mem_...”
- Explicit capture skills can still create direct memories when clearly requested.

### Milestone 6 — MCP Candidate Tools

Deliverables:

- `vestige_propose_candidate`
- `vestige_list_candidates`
- `vestige_get_candidate`
- Approval/rejection tools deferred or disabled by default.

Acceptance criteria:

- MCP agents can propose candidates.
- MCP agents can inspect pending candidates.
- MCP cannot approve candidates unless explicitly configured.

### Milestone 7 — Docs and Demo

Deliverables:

- README V0.2 section.
- PRD linked from main README/CLAUDE.
- Landing page roadmap updated if needed.
- Example workflow.

Acceptance criteria:

- New user understands:
  - direct memory vs candidate memory
  - how to inspect inbox
  - how to approve/reject
  - how skills use the inbox

## 18. Testing Requirements

### Unit Tests

- Candidate ID parsing.
- Candidate status transitions.
- Rejection reason parsing.
- Candidate-to-memory conversion.
- Dedup helper behaviour.

### Store Integration Tests

- Candidate create/list/get.
- Project-scoped candidate isolation.
- Approve creates memory.
- Approve indexes memory in FTS.
- Reject excludes from recall.
- Duplicate relationships persist.
- Candidate source/event links persist.

### CLI Smoke Tests

- `candidate add → inbox → inbox show`.
- `candidate add → approve → recall`.
- `candidate add → reject → recall absent`.
- `--json` output shapes.
- Duplicate hint display.

### MCP Tests

- `vestige_propose_candidate`.
- `vestige_list_candidates`.
- `vestige_get_candidate`.
- Approval disabled by default returns structured error if approval tool exists.

### Regression Tests

- Normal `vestige search` does not return pending candidates.
- Normal `vestige context` does not include pending candidates.
- Rejected candidates never leak into recall.
- Approved candidates appear as memories, not candidates.

## 19. Acceptance Criteria

V0.2 is complete when:

- A candidate memory can be created from CLI.
- Pending candidates can be listed.
- Candidate detail can be inspected.
- A candidate can be approved into a normal memory.
- A candidate can be rejected.
- Approved candidates appear in search/recall.
- Pending/rejected candidates do not appear in normal search/recall.
- Candidate approval preserves source/candidate provenance.
- Auto-memorise proposes candidates instead of writing durable memories.
- MCP can propose and inspect candidates.
- MCP approval is disabled by default.
- Dedup hints exist for active memories and pending candidates.
- All new flows support JSON output.
- Tests cover candidate lifecycle and recall isolation.

## 20. Open Questions

1. Should V0.2 reuse `memory_events` for raw events or introduce a dedicated `raw_events` table?
2. Should `candidate add` be a public command, or should candidates only be created from `event record` / MCP / skills?
3. Should `approve --edit` open `$EDITOR`, accept inline flags, or both?
4. Should explicit “remember this” commands still write direct memories, or also go through candidates by default?
5. Should candidates have embeddings before approval, or only use lexical dedup until promoted?
6. Should rejected candidates be searchable through a special inbox/history command?
7. Should approval be allowed through MCP with config, or kept CLI-only until V0.3?
8. How much raw source content should candidates store?
9. Should duplicate candidates be auto-suppressed or shown with a duplicate warning?
10. Should project config allow disabling candidate categories, such as notes but not decisions?

## 21. Recommended First Slice

Build this first:

```bash
vestige candidate add --type note --body "README install section is stale."
vestige inbox
vestige inbox show cand_...
vestige approve cand_...
vestige reject cand_...
```

No MCP. No skills. No event abstraction beyond minimal source fields.

Once that works end-to-end, update `vestige-auto-memorise` to write candidates instead of memories.

That keeps the first implementation small and proves the real product loop before widening the surface.
