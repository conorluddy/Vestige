# Vestige PRD

## 1. Product Summary

Vestige is a local-first, repo-pinned memory layer for coding agents.

It lets agents leave durable, inspectable traces inside a project scope, then recall those traces later through CLI and MCP without polluting unrelated projects.

Vestige is not a chatbot, note-taking app, vector database, or agent framework. It is memory infrastructure for long-lived local agents.

## 2. Product Thesis

Modern coding agents lose useful context between sessions. They repeatedly rediscover project decisions, naming conventions, architecture constraints, user preferences, and unresolved questions. Existing memory approaches often collapse everything into a global vector soup, causing stale recall, context pollution, and poor trust.

Vestige solves this by making project memory explicit, scoped, progressively disclosed, source-linked, and inspectable.

The core idea:

```txt
A repo can leave useful traces.
An agent can recall those traces later.
A human can inspect and control them.
```

## 3. Target Users

### Primary V0 User

Solo developers and agent-heavy builders who work with local coding agents across one or more repositories.

Examples:

- Developers using Claude Code, Cursor, Codex, local agents, or custom MCP-enabled workers.
- Indie developers managing multiple long-lived side projects.
- Engineers who want agents to remember project-specific decisions without contaminating unrelated repos.

### Later Users

- Teams working across many repos and microservices.
- Agent framework authors.
- Developers running persistent local agent environments.
- Privacy-conscious users who want local-first memory rather than hosted memory.

## 4. Core Positioning

### Short Positioning

Vestige is repo-pinned memory for coding agents.

### Fuller Positioning

Vestige gives local agents durable, inspectable project memory through CLI and MCP. It stores project traces in a local SQLite-backed memory store, supports progressive memory disclosure, and keeps project memories isolated by default.

### Differentiators

- Project-scoped by default.
- Local-first.
- SQLite as canonical store.
- Embeddings are optional, rebuildable indexes.
- Progressive memory disclosure is built in from the start.
- CLI and MCP first.
- Human inspectability and deletion are first-class.
- Cross-project federation is future opt-in, not default pollution.

## 5. Design Principles

### 5.1 Project Memory First

Vestige should default to project-pinned memory.

A memory created inside one repo should not automatically affect another repo.

Global memory should exist only for durable user/tooling preferences and should be explicitly included in recall.

### 5.2 Progressive Memory Disclosure

Memory retrieval should return compact handles first, not full blobs.

Each promoted memory must support several representations:

```txt
L0: handle
L1: title / one-liner
L2: summary
L3: compressed body
L4: full body
L5: source evidence
```

Agents should explicitly expand memory depth when needed.

### 5.3 Source of Truth Separation

Vestige should separate raw evidence, derived memory, and indexes.

```txt
Raw events / source evidence = durable journal
Derived memories = replaceable interpretation
Indexes / embeddings = disposable acceleration layer
```

### 5.4 Explicit Capture Before Automatic Capture

V0 should prioritise explicit memory recording over automatic ingestion.

The system should first prove that useful project traces can be manually or agent-explicitly recorded, recalled, expanded, and forgotten.

Automatic extraction and assimilation can come later through a review inbox.

### 5.5 Human Control

The user should always be able to answer:

```txt
What was stored?
Where is it stored?
Why was it returned?
How do I expand it?
How do I remove it?
```

### 5.6 Agent-Safe Defaults

MCP should expose high-level memory tools, not raw database access.

Agents should not have destructive powers by default.

### 5.7 Federate Later, Do Not Centralise Early

Project memory stores should remain authoritative and isolated.

A later federation layer may index compact representations across projects, but it should not merge or pollute individual project stores by default.

## 6. V0 Product Goal

V0 should prove the smallest useful loop:

```txt
1. Initialise Vestige inside a repo.
2. Record explicit project memories.
3. Recall compact memory results later.
4. Expand selected memories by depth.
5. Provide project context to agents through MCP.
6. Let the user inspect and forget stored memories.
```

V0 is successful when a local coding agent can start a new session in a repo, call Vestige through MCP, retrieve relevant project context, and continue work without asking the user to restate known decisions.

## 7. V0 Non-Goals

V0 should not include:

- Cross-project federation.
- Dream jobs.
- Decay scoring.
- Automatic conversation ingestion.
- Global super-index.
- Swift menu bar app.
- Cloud sync.
- Encryption at rest.
- Complex permissions UI.
- Automatic contradiction detection.
- Required daemon/background service.
- Hosted/cloud mode.
- Fancy graphical UI.
- Full vector database functionality.
- General note-taking features.

## 8. V0 Runtime Model

V0 should not require a background daemon.

The initial runtime should be:

```txt
vestige CLI
vestige MCP server
SQLite project store
```

The MCP process may read/write the project SQLite store directly in V0.

A daemon can be introduced later when background indexing, lifecycle scheduling, concurrency control, and cross-project registry become valuable.

## 9. Storage Model

### 9.1 Default Storage Layout

Vestige should avoid storing private memory databases directly inside the Git repo by default.

Recommended V0 layout:

```txt
Repo:
  .vestige/config.toml

Machine:
  ~/.vestige/projects/<project-id>/memory.sqlite
```

The repo-local config pins the repository to a project memory scope.

The private SQLite database lives in the user data directory.

### 9.2 Why This Layout

Benefits:

- Avoids accidental commits of private memory.
- Keeps project memory scoped.
- Allows agents to detect project memory from the repo.
- Makes project DBs easy to archive/delete.
- Leaves room for later federation across project DBs.

### 9.3 Project Identity

When `vestige init` runs inside a Git repo, Vestige should identify the project using:

1. Explicit project name if provided.
2. Git remote URL hash if available.
3. Git root path hash as fallback.
4. Folder name as human-readable display name.

The project ID should be stable for the same repo on the same machine.

## 10. `vestige init`

### 10.1 Purpose

`vestige init` bootstraps Vestige memory into the current repository.

It creates a project memory scope, writes minimal repo-local config, creates the local SQLite store, and registers the project with the local Vestige environment.

### 10.2 Command

```bash
vestige init
```

Optional V0 flags:

```bash
vestige init --name "My Project"
vestige init --summary "Short project description"
vestige init --dry-run
```

Deferred flags:

```bash
vestige init --local
vestige init --workspace
vestige init --global
vestige init --no-files
```

### 10.3 Created Files

V0 should create:

```txt
.vestige/
└── config.toml
```

Optional later files:

```txt
.vestige/context.md
.vestige/decisions.md
.vestige/README.md
```

### 10.4 Example Config

```toml
project_id = "vestige"
project_name = "Vestige"
scope = "project"

[storage]
mode = "user_data"
path = "~/.vestige/projects/vestige/memory.sqlite"

[recall]
default_depth = "one_liner"
max_results = 8
include_global_preferences = false

[mcp]
allow_record_observation = true
allow_record_decision = true
allow_forget = false
```

### 10.5 Acceptance Criteria

- Running `vestige init` inside a Git repo creates `.vestige/config.toml`.
- The command creates a SQLite DB under `~/.vestige/projects/<project-id>/memory.sqlite`.
- Running `vestige status` after init shows the active project scope.
- Running `vestige init --dry-run` shows planned actions without writing files.
- Running `vestige init` twice should be safe and idempotent.

## 11. Memory Model

### 11.1 Memory Types

V0 should support:

```txt
observation
note
decision
preference
project_summary
open_question
```

Later types:

```txt
source_event
candidate_memory
entity
relationship
pattern
contradiction
cross_project_candidate
```

### 11.2 Memory Statuses

V0 should support:

```txt
active
deleted
```

V0.1+ should add:

```txt
candidate
pinned
archived
superseded
contradicted
```

### 11.3 Required Representations

Every promoted memory must have:

```txt
title
one_liner
summary
compressed_body
full_body
```

For V0, these can be manually provided or derived using simple deterministic heuristics.

LLM-generated representation creation can be added later.

### 11.4 Representation Definitions

#### Title

Short display label.

Example:

```txt
SQLite as canonical store
```

#### One-Liner

Single sentence enough to judge relevance.

Example:

```txt
SQLite should store durable memory while vector indexes remain replaceable.
```

#### Summary

Human-friendly paragraph explaining the memory.

Example:

```txt
The project should use SQLite as the canonical local store for memories, with embeddings and vector indexes treated as rebuildable retrieval infrastructure rather than the source of truth.
```

#### Compressed Body

Dense agent-friendly version with filler removed.

Example:

```txt
Decision: SQLite canonical store. Vector layer non-authoritative/rebuildable. Rationale: durability, migrations, provenance, local-first portability. Possible indexes: FTS5, sqlite-vec, LanceDB/Qdrant later.
```

#### Full Body

Full memory content, including rationale and nuance.

### 11.5 Suggested SQLite Tables

V0 schema should remain simple but extensible.

```sql
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root_path TEXT,
  git_remote TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  type TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  confidence REAL NOT NULL DEFAULT 1.0,
  importance REAL NOT NULL DEFAULT 0.5,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT,
  FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE TABLE memory_representations (
  id TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL,
  representation_type TEXT NOT NULL,
  content TEXT NOT NULL,
  token_count INTEGER,
  content_hash TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (memory_id) REFERENCES memories(id)
);

CREATE TABLE memory_sources (
  id TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL,
  source_type TEXT NOT NULL,
  source_ref TEXT,
  source_content TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (memory_id) REFERENCES memories(id)
);

CREATE TABLE memory_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  payload_json TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id)
);
```

FTS support should be added over memory representations.

Example:

```sql
CREATE VIRTUAL TABLE memory_fts USING fts5(
  memory_id UNINDEXED,
  representation_type UNINDEXED,
  content
);
```

## 12. CLI Requirements

### 12.1 Required V0 Commands

```bash
vestige init
vestige status
vestige remember
vestige note add
vestige decision add
vestige preference add
vestige question add
vestige list
vestige search
vestige recall
vestige show
vestige forget
vestige mcp
```

### 12.2 `vestige status`

Shows current Vestige project state.

Example output:

```txt
Project: Vestige
Scope: project
Git root: /Users/conor/code/vestige
Config: .vestige/config.toml
Memory DB: ~/.vestige/projects/vestige/memory.sqlite
Memories: 12 active, 0 deleted
MCP: available
```

### 12.3 `vestige remember`

Generic memory capture.

```bash
vestige remember "Use SQLite as canonical store; vector indexes are replaceable."
```

Default type: `note` or `observation`.

### 12.4 `vestige decision add`

Records a project decision.

```bash
vestige decision add "MCP should be a thin adapter over the memory engine."
```

Optional flags:

```bash
vestige decision add "..." --rationale "..." --importance 0.8
```

### 12.5 `vestige search`

Searches active project memory.

```bash
vestige search "MCP adapter"
```

Default output should use one-liners.

Example:

```txt
mem_01  decision  MCP as thin adapter
        MCP should wrap Vestige memory operations without owning storage/lifecycle logic.

mem_02  note      MCP tools for progressive disclosure
        Agents should search compact memory cards first, then expand selected memories.
```

### 12.6 `vestige recall`

Similar to search, but intended for agent/user context recall.

```bash
vestige recall "what do we know about project architecture?"
```

It may apply more opinionated ranking/filtering than raw search.

### 12.7 `vestige show`

Expands a memory by depth.

```bash
vestige show mem_01
vestige show mem_01 --depth one_liner
vestige show mem_01 --depth summary
vestige show mem_01 --depth compressed
vestige show mem_01 --depth full
vestige show mem_01 --sources
```

### 12.8 `vestige list`

Lists active memories.

```bash
vestige list
vestige list --type decision
vestige list --type open_question
```

### 12.9 `vestige forget`

Soft-deletes a memory.

```bash
vestige forget mem_01
```

V0 behaviour:

- Mark status as deleted.
- Set deleted_at.
- Remove from default search/recall.
- Remove from FTS index if implemented.

Hard delete can come later.

### 12.10 `vestige mcp`

Starts the MCP server.

```bash
vestige mcp
```

The MCP server should resolve the current project from `.vestige/config.toml` or nearest Git root.

## 13. MCP Requirements

### 13.1 MCP Philosophy

MCP should be a thin agent-facing adapter over Vestige memory operations.

It should expose high-level tools, not raw SQL or unrestricted database mutation.

### 13.2 V0 MCP Tools

Required:

```txt
vestige_bootstrap
vestige_search
vestige_expand
vestige_get_project_context
vestige_record_observation
vestige_record_decision
```

Optional V0:

```txt
vestige_record_open_question
vestige_list_recent
```

Deferred:

```txt
vestige_forget
vestige_check_contradictions
vestige_record_preference
vestige_dream
vestige_search_federated
```

### 13.3 `vestige_bootstrap`

Purpose:

Return compact standing context for the current project.

Input:

```json
{
  "max_items": 8,
  "include": ["summary", "decisions", "open_questions"]
}
```

Output:

```json
{
  "project": {
    "id": "vestige",
    "name": "Vestige"
  },
  "context": "Compact context pack...",
  "memories": []
}
```

### 13.4 `vestige_search`

Purpose:

Search project memory and return compact memory cards.

Input:

```json
{
  "query": "MCP adapter progressive disclosure",
  "limit": 8,
  "types": ["decision", "note"],
  "depth": "one_liner"
}
```

Output:

```json
{
  "results": [
    {
      "id": "mem_01",
      "type": "decision",
      "title": "MCP as thin adapter",
      "one_liner": "MCP should wrap Vestige operations without owning storage or lifecycle logic.",
      "score": 0.91,
      "available_depths": ["summary", "compressed", "full", "sources"]
    }
  ]
}
```

### 13.5 `vestige_expand`

Purpose:

Expand selected memory by depth.

Input:

```json
{
  "memory_id": "mem_01",
  "depth": "compressed"
}
```

Output:

```json
{
  "id": "mem_01",
  "type": "decision",
  "title": "MCP as thin adapter",
  "depth": "compressed",
  "content": "Decision: MCP thin adapter. Storage/lifecycle owned by Vestige core. Avoid embedding durable logic in MCP process."
}
```

### 13.6 `vestige_get_project_context`

Purpose:

Return a compact project context pack for agents.

Input:

```json
{
  "include": ["summary", "decisions", "open_questions"],
  "budget_tokens": 1200
}
```

Output:

```json
{
  "context_pack": "Project: Vestige\nSummary: ...\nCurrent decisions: ...\nOpen questions: ..."
}
```

### 13.7 `vestige_record_observation`

Purpose:

Record a low-to-medium confidence project observation.

Input:

```json
{
  "content": "The project currently favours repo-pinned memory by default.",
  "importance": 0.5,
  "source": {
    "type": "agent_session",
    "ref": "current"
  }
}
```

### 13.8 `vestige_record_decision`

Purpose:

Record an explicit project decision.

Input:

```json
{
  "decision": "Vestige V0 should not require a background daemon.",
  "rationale": "A CLI plus MCP process is enough to prove the core loop and reduces initial implementation complexity.",
  "importance": 0.8,
  "source": {
    "type": "agent_session",
    "ref": "current"
  }
}
```

### 13.9 MCP Safety Defaults

V0 MCP should not expose destructive tools by default.

Agents may:

- Search memory.
- Expand memory.
- Retrieve project context.
- Record observations.
- Record decisions.

Agents may not by default:

- Hard delete memory.
- Modify existing memory.
- Search unrelated projects.
- Access raw source evidence unless explicitly expanded by tool design.
- Run arbitrary SQL.

## 14. Search and Recall

### 14.1 V0 Search

V0 should use SQLite FTS5 and metadata filtering.

Embeddings are not required for skeleton V0.

Search should include:

- Active memories only by default.
- Current project scope only by default.
- One-liner output by default.
- Type filters.
- Depth expansion through separate command/tool.

### 14.2 V0 Ranking

Initial ranking can be simple:

```txt
FTS score
+ importance
+ memory type boost
+ recency boost
```

Decision and project_summary memories may receive a small boost for project context queries.

### 14.3 Later Hybrid Search

V0.1+ should support embeddings through a replaceable provider interface.

Potential embedding targets:

```txt
title
one_liner
summary
compressed_body
full_body
```

Search should eventually combine:

```txt
FTS
semantic similarity
entity/project scope
importance
confidence
recency
usage/reinforcement
```

## 15. Project Context Pack

### 15.1 Purpose

Project context is the most important V0 agent feature.

It gives an agent enough compact memory to continue useful work in a repo.

### 15.2 Included Sections

V0 context pack should include:

```txt
Project name
Project summary
Current decisions
Open questions
Recent important memories
```

Later:

```txt
Known constraints
Architecture summary
Useful commands
Linked project memories
Stale assumptions
Contradictions
```

### 15.3 Example Output

```txt
Project: Vestige

Summary:
Vestige is a local-first, repo-pinned memory layer for coding agents.

Current decisions:
- V0 should be CLI + MCP + SQLite, with no required daemon.
- Project memory is scoped per repo by default.
- Private DBs live under ~/.vestige/projects/<project-id>/.
- Progressive disclosure is mandatory for every memory.
- MCP should expose high-level memory tools, not raw SQL.

Open questions:
- Which language/runtime should implement the first CLI?
- Should embeddings be added in V0.1 or V0?
- How much source evidence should V0 store?
```

## 16. Human Inspectability

V0 must make memory inspection easy.

Required commands:

```bash
vestige status
vestige list
vestige search
vestige show
vestige forget
```

Later commands:

```bash
vestige why mem_01
vestige sources mem_01
vestige trace query_01
vestige inbox
vestige approve cand_01
vestige reject cand_01
```

## 17. Forgetting

### 17.1 V0 Forget

V0 should implement soft delete.

Soft-deleted memories should:

- Be excluded from normal list/search/recall.
- Retain a deleted status and timestamp.
- Be restorable later if restore is added.

### 17.2 Later Forget

Future versions should support:

```txt
archive
soft delete
hard delete
forget source
forget project
forget all derived memories from source
```

Important future rule:

```txt
Forgotten memories must not be regenerated from old summaries or indexes.
```

## 18. Agent-Friendly Implementation Notes

### 18.1 Recommended Build Order

Agents implementing V0 should work in this order:

```txt
1. CLI project bootstrap
2. SQLite schema/migrations
3. Memory creation commands
4. Representation handling
5. List/show/search commands
6. Forget command
7. Project context generation
8. MCP server
9. Tests and fixtures
```

### 18.2 Preferred Constraints

- Keep V0 small.
- Avoid background daemons.
- Avoid automatic ingestion.
- Avoid cross-project search.
- Avoid hard delete.
- Avoid embeddings until core loop works.
- Keep all commands scriptable and deterministic.
- Store data locally.
- Make DB path visible.
- Make project scope explicit.

### 18.3 Agent Task Boundaries

Good implementation tasks:

```txt
- Add `vestige init` with config generation.
- Add SQLite migration runner.
- Add memory insert/list/show commands.
- Add FTS indexing.
- Add project context renderer.
- Add MCP tool `vestige_search`.
```

Bad implementation tasks:

```txt
- Build complete lifecycle system.
- Build cross-project dream engine.
- Add full vector DB abstraction before basic recall works.
- Implement GUI.
- Add cloud sync.
- Add complex permissions.
```

## 19. V0 Milestones

### Milestone 0: Repo Bootstrap

Deliverables:

- `vestige init`
- `.vestige/config.toml`
- project ID generation
- SQLite DB creation
- `vestige status`

Acceptance criteria:

- A repo can be initialised.
- The project scope is visible.
- The DB path is visible.
- Init is idempotent.

### Milestone 1: Memory Core

Deliverables:

- Add observation/note.
- Add decision.
- Add preference.
- List memories.
- Show memory.
- Soft delete memory.

Acceptance criteria:

- User can record and inspect memories from CLI.
- Deleted memories do not appear in normal recall.

### Milestone 2: Progressive Representations

Deliverables:

- Required representation fields.
- Depth-based display.
- Basic deterministic representation generation.

Acceptance criteria:

- Every memory can be shown as one-liner, summary, compressed, or full.

### Milestone 3: Search and Recall

Deliverables:

- FTS5 index.
- `vestige search`.
- `vestige recall`.
- Type filters.
- Active project scope default.

Acceptance criteria:

- User can search project memory.
- Results return compact memory cards.
- User can expand selected results.

### Milestone 4: Project Context

Deliverables:

- `vestige project context` or `vestige context`.
- Project summary handling.
- Decisions section.
- Open questions section.

Acceptance criteria:

- Agent-readable project context can be generated from stored memories.

### Milestone 5: MCP Adapter

Deliverables:

- `vestige mcp`.
- `vestige_bootstrap`.
- `vestige_search`.
- `vestige_expand`.
- `vestige_get_project_context`.
- `vestige_record_observation`.
- `vestige_record_decision`.

Acceptance criteria:

- MCP-compatible agents can retrieve project context.
- MCP-compatible agents can record observations and decisions.
- MCP defaults to the current repo scope.

## 20. Roadmap Stub

### V0: Skeleton Project Memory

Focus:

- Repo init.
- SQLite memory store.
- Explicit capture.
- Progressive disclosure.
- FTS recall.
- Project context pack.
- Minimal MCP.

### V0.1: Embeddings and Hybrid Search

Add:

- Embedding provider interface.
- Embeddings for summary/compressed/full representations.
- Hybrid FTS + vector search.
- Reindex command.
- Embedding model/version metadata.

Commands:

```bash
vestige embed
vestige reindex
vestige search --semantic
```

### V0.2: Assimilation Inbox

Add:

- Raw event capture.
- Candidate memories.
- Review inbox.
- Approve/reject flow.

Commands:

```bash
vestige inbox
vestige approve cand_01
vestige reject cand_01
```

### V0.3: Provenance and Receipts

Add:

- Stronger source model.
- `vestige why`.
- `vestige sources`.
- Query trace logs.

Commands:

```bash
vestige why mem_01
vestige sources mem_01
vestige trace query_01
```

### V0.4: Daemon Runtime

Add:

- `vestiged` or `vestige serve`.
- Local API.
- MCP talks to daemon.
- Better concurrency control.
- Optional macOS LaunchAgent support.

Commands:

```bash
vestige serve
vestige service install
vestige service start
vestige service stop
```

### V0.5: Dream-Lite Consolidation

Add:

- Project summary refresh.
- Duplicate memory clustering.
- Candidate summary generation.
- Review-first consolidation.

Command:

```bash
vestige dream
```

### V0.6: Global Preferences

Add:

- `~/.vestige/global.sqlite`.
- Global user/tooling preferences.
- Explicit global recall inclusion.
- Project memory wins over global memory.

Commands:

```bash
vestige preference add --global "Prefer Markdown PRDs"
vestige search --include-global "style"
```

### V0.7: Federated Cross-Project Index

Add:

- Project registry.
- Federation DB.
- Cross-project search over compact representations.
- Related memory discovery.
- No project pollution by default.

Commands:

```bash
vestige projects list
vestige search --all-projects "SQLite memory"
vestige related
```

### V1: Responsible Long-Lived Memory

V1 should include:

- Project-pinned memory.
- Optional global preferences.
- MCP integration.
- Progressive disclosure.
- Source-linked memories.
- Hybrid search.
- Review inbox.
- Basic dream/consolidation.
- Forget/archive.
- Explicit cross-project federation.

## 21. Open Questions

These should be resolved before or during V0 implementation:

1. What implementation language should V0 use?
2. Should V0 include embeddings, or should embeddings wait until V0.1?
3. Should memory IDs be ULIDs, UUIDs, or deterministic hashes?
4. Should `vestige remember` create all representations automatically using heuristics?
5. Should `vestige decision add` require a rationale?
6. How should MCP resolve the current project when launched outside a repo?
7. Should `.vestige/config.toml` be intended for commit or local-only use?
8. Should V0 store source content verbatim or only source metadata?
9. Should deleted memories be restorable in V0?
10. Should project summaries be manually maintained or generated from memories?
11. What is the minimum viable MCP config for Claude Code / local workers?
12. Should `vestige mcp` support read-only mode?
13. Should global preferences exist in V0 or wait until V0.6?
14. Should the first public README emphasise CLI, MCP, or project memory?

## 22. Suggested README Demo

```bash
brew install vestige

cd ~/code/my-project
vestige init --name "My Project" --summary "An app for tracking useful things."

vestige decision add "Use SQLite as the canonical local store."
vestige note add "MCP should be a thin adapter over the memory engine."
vestige question add "Should embeddings ship in V0.1 or V0?"

vestige recall "architecture decisions"
vestige context
vestige mcp
```

Agent flow:

```txt
1. Agent starts inside repo.
2. Agent calls `vestige_get_project_context`.
3. Agent receives compact project memory.
4. Agent continues work using stored decisions.
5. Agent records new decisions through `vestige_record_decision`.
6. User inspects memory with `vestige list` and `vestige show`.
```

## 23. Definition of Done for V0

V0 is complete when:

- A repo can be initialised with Vestige.
- A project-specific SQLite memory store is created.
- The user can record decisions, notes, preferences, and open questions.
- Every memory supports progressive disclosure fields.
- The user can search and recall project memories.
- The user can expand memories by depth.
- The user can soft-delete memories.
- A project context pack can be generated.
- An MCP-compatible agent can retrieve project context.
- An MCP-compatible agent can record observations and decisions.
- Default recall does not search unrelated projects.
- The system clearly exposes where memory is stored.

## 24. Final V0 Scope Statement

Vestige V0 is a CLI-first, SQLite-backed, repo-pinned memory layer for coding agents.

It initialises memory per repository, records explicit project traces, retrieves compact memory cards, expands memories progressively, generates project context packs, and exposes a minimal MCP interface for local agents.

V0 proves repo-pinned recall. Later versions earn long-lived autonomous memory.

