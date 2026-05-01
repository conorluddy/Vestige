# Vestige V0.1 PRD — Embeddings and Hybrid Recall

## 1. Product Summary

Vestige V0.1 adds semantic recall to the existing repo-pinned memory system.

The goal is to improve retrieval quality without weakening the trust model established in V0:

```txt
SQLite memories remain canonical.
FTS search remains available.
Embeddings are optional, rebuildable indexes.
Progressive disclosure remains the default.
Project scope remains the default boundary.
```

V0.1 should not expand Vestige into a dream engine, daemon, global memory system, or cross-project federation layer. It should only add a reliable semantic retrieval substrate.

## 2. V0.1 Thesis

```txt
V0.1 adds semantic recall without turning memory into an opaque vector soup.
```

Embeddings should help agents find relevant memories when exact keyword search is insufficient, while still returning compact memory cards that can be expanded deliberately.

The core loop:

```txt
1. Memory exists in project SQLite store.
2. Selected memory representations are embedded.
3. User/agent searches semantically or with hybrid search.
4. Vestige returns compact memory cards.
5. User/agent expands only selected memories.
6. Existing inspectability, deletion, and project scoping still work.
```

## 3. Target User

Same as V0:

- Solo developers using local coding agents.
- Agent-heavy builders using Claude Code, Cursor, Codex, or MCP-enabled local workers.
- Developers who want project memory to survive across agent sessions.

V0.1 specifically benefits users once their project memory grows beyond what simple keyword search handles well.

## 4. V0.1 Goals

V0.1 should provide:

```txt
- Embedding storage linked to memory representations.
- An embedding provider abstraction.
- A deterministic test embedding provider.
- sqlite-vec-backed vector search, if practical.
- CLI commands for embedding and reindexing.
- Search modes: lexical, semantic, hybrid.
- Hybrid recall that merges FTS and vector results.
- MCP support for search mode selection.
- Score diagnostics for inspectability.
```

## 5. V0.1 Non-Goals

V0.1 should not include:

```txt
- Dream jobs.
- Decay scoring.
- Assimilation inbox.
- Automatic conversation ingestion.
- Cross-project federation.
- Global preferences.
- Background daemon.
- Scheduled embedding jobs.
- Remote embedding provider as default.
- Cloud sync.
- GUI/menu bar app.
- Complex ranking UI.
- Memory mutation through MCP beyond existing V0 tools.
```

## 6. Design Principles

### 6.1 Embeddings Are Not Source of Truth

Embeddings must be treated as rebuildable indexes over canonical memory representations.

If embeddings are deleted, stale, or unavailable, Vestige should still work through FTS and direct memory inspection.

### 6.2 Representation-Level Embeddings

Vestige memories already support progressive disclosure. Embeddings should respect that model.

Do not embed only a single opaque memory blob.

Embeddings should be linked to memory representations:

```txt
title
one_liner
summary
compressed_body
full_body
```

V0.1 default should embed:

```txt
summary
compressed_body
```

`full_body` embeddings can be optional/deferred.

### 6.3 Local-First Default

Vestige should not require a remote embedding API to function.

V0.1 must support a deterministic local/test provider. A real local provider such as Ollama may be added if practical, but remote providers should be explicit opt-in later.

### 6.4 Progressive Disclosure Remains Default

Search results should still return compact memory cards by default.

Even semantic results should return:

```txt
id
type
title
one_liner
score
available_depths
```

Full memory content should not be injected automatically.

### 6.5 Inspectable Ranking

Hybrid search should expose enough score diagnostics to explain why results appeared.

JSON output should include score components where practical.

## 7. Existing V0 Baseline

V0 is assumed to already include:

```txt
- `vestige init`
- repo-pinned project scope
- SQLite project store
- memory CRUD-ish commands
- soft delete / restore
- progressive memory representations
- FTS5 search/recall
- project context pack
- MCP server
- clean test suite
```

V0.1 must preserve all existing V0 behaviours.

## 8. Storage Requirements

### 8.1 Embedding Metadata

Add storage for embeddings linked to memory representations.

Suggested table:

```sql
CREATE TABLE memory_embeddings (
  id TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL,
  representation_id TEXT NOT NULL,
  representation_type TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  dimensions INTEGER NOT NULL,
  vector_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  stale_at TEXT,
  FOREIGN KEY (memory_id) REFERENCES memories(id),
  FOREIGN KEY (representation_id) REFERENCES memory_representations(id)
);
```

### 8.2 Embedding Jobs

Add a basic job/status table so failed embedding attempts are inspectable.

Suggested table:

```sql
CREATE TABLE embedding_jobs (
  id TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL,
  representation_id TEXT NOT NULL,
  representation_type TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL,
  error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (memory_id) REFERENCES memories(id),
  FOREIGN KEY (representation_id) REFERENCES memory_representations(id)
);
```

Job statuses:

```txt
pending
completed
failed
skipped
```

### 8.3 Vector Storage

If using sqlite-vec, create vector storage isolated behind the store/index abstraction.

Implementation details may vary depending on sqlite-vec API constraints, but the vector index must be keyed to:

```txt
memory_id
representation_id
representation_type
provider
model
dimensions
```

### 8.4 Staleness

Embeddings should be considered stale when:

```txt
- the representation content changes
- the embedding provider changes
- the embedding model changes
- dimensions change
```

V0.1 may detect staleness without automatically fixing it.

Required command should make stale state visible:

```bash
vestige embeddings status
```

## 9. Embedding Provider Abstraction

### 9.1 Required Trait / Interface

Add a provider abstraction in the core/store layer.

Suggested Rust shape:

```rust
pub trait EmbeddingProvider {
    fn provider_name(&self) -> &'static str;
    fn model_name(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed(&self, input: &str) -> Result<Vec<f32>>;
}
```

The exact error type can follow the existing project conventions.

### 9.2 Required V0.1 Provider

V0.1 must include a deterministic test/local provider.

Example:

```txt
FakeEmbeddingProvider
```

Requirements:

```txt
- no network access
- deterministic output for a given input
- stable vector dimensions
- suitable for tests and local smoke checks
```

This provider is not expected to produce high-quality semantic embeddings. It exists to prove the storage, indexing, and search pipeline.

### 9.3 Optional V0.1 Provider

Optional if implementation cost is low:

```txt
OllamaEmbeddingProvider
```

Requirements if included:

```txt
- disabled by default
- configured explicitly
- errors handled cleanly
- no hard dependency on Ollama for test suite
```

### 9.4 Deferred Providers

Deferred until after V0.1:

```txt
OpenAI
Voyage
Cohere
local sentence-transformer runner
other hosted providers
```

Remote providers must be explicit opt-in because Vestige is local-first.

## 10. Search Modes

Vestige should support three search modes.

### 10.1 Lexical Search

```txt
mode = lexical
```

Uses existing FTS5 search only.

This should remain available at all times.

### 10.2 Semantic Search

```txt
mode = semantic
```

Uses vector search only.

If no embeddings exist, semantic search should return a clear error or empty result with a helpful message.

Example:

```txt
No embeddings found for this project. Run `vestige embed --all` first.
```

### 10.3 Hybrid Search

```txt
mode = hybrid
```

Combines FTS and vector results.

Hybrid search should:

```txt
- run FTS search
- run semantic search when embeddings exist
- merge result sets
- deduplicate by memory_id
- apply simple ranking
- return compact memory cards
```

If embeddings are unavailable, hybrid may fall back to lexical search with a warning/metadata field.

## 11. Hybrid Ranking

### 11.1 Initial Ranking Formula

Start simple and inspectable.

Suggested formula:

```txt
hybrid_score =
  fts_score_normalized * 0.55
  + vector_score_normalized * 0.35
  + importance_boost * 0.07
  + type_boost * 0.03
```

Exact weights can change, but V0.1 should keep the formula simple and documented.

### 11.2 Type Boost

Suggested memory type boosts:

```txt
project_summary: high for context queries
decision: medium-high
preference: medium
open_question: medium
note/observation: neutral
```

### 11.3 Score Diagnostics

JSON output should include score parts where practical.

Example:

```json
{
  "id": "mem_01",
  "type": "decision",
  "title": "MCP as thin adapter",
  "one_liner": "MCP should wrap Vestige operations without owning storage or lifecycle logic.",
  "score": 0.87,
  "score_parts": {
    "fts": 0.72,
    "vector": 0.91,
    "importance": 0.5,
    "type_boost": 0.1
  }
}
```

Human-readable output does not need to show all score parts by default.

## 12. CLI Requirements

### 12.1 New Commands

V0.1 should add:

```bash
vestige embed
vestige reindex
vestige embeddings status
```

V0.1 should extend:

```bash
vestige search
vestige recall
```

### 12.2 `vestige embed`

Embeds memories or representations.

Required forms:

```bash
vestige embed --all
vestige embed --memory <memory-id>
vestige embed --dry-run
```

Optional forms:

```bash
vestige embed --representation summary
vestige embed --representation compressed_body
vestige embed --provider fake
vestige embed --model test-small
```

Default behaviour:

```txt
- embed active memories only
- embed summary and compressed_body representations
- skip deleted memories
- skip unchanged representations that already have current embeddings
```

### 12.3 `vestige embeddings status`

Shows embedding/index state for the active project.

Example output:

```txt
Project: Vestige
Provider: fake
Model: test-small
Dimensions: 64

Memories: 42 active
Embeddable representations: 84
Embedded representations: 80
Stale embeddings: 2
Failed jobs: 1
Missing embeddings: 4
```

JSON output should be supported if existing CLI has JSON conventions.

### 12.4 `vestige reindex`

Rebuilds indexes.

Required:

```bash
vestige reindex --fts
vestige reindex --embeddings
```

Optional:

```bash
vestige reindex --all
```

Behaviour:

```txt
- `--fts` rebuilds FTS index
- `--embeddings` rebuilds embedding/vector index
- should not mutate canonical memory content
```

### 12.5 Search Mode Flags

Extend `vestige search` and `vestige recall`:

```bash
vestige search "query" --mode lexical
vestige search "query" --mode semantic
vestige search "query" --mode hybrid
```

Convenience aliases are acceptable:

```bash
vestige search "query" --lexical
vestige search "query" --semantic
vestige search "query" --hybrid
```

Default for V0.1:

```txt
Existing default behaviour should remain lexical unless config explicitly enables hybrid default.
```

This avoids surprising users.

### 12.6 Search Output

Human-readable output should remain compact.

Example:

```txt
mem_01  decision  MCP as thin adapter        score 0.87
        MCP should wrap Vestige operations without owning storage/lifecycle logic.

mem_02  note      Progressive disclosure     score 0.81
        Search should return compact memory cards before expanding full memory.
```

JSON output should include score parts and search mode metadata.

## 13. MCP Requirements

### 13.1 Existing MCP Tools

V0.1 should preserve existing MCP tools.

Known V0 tools may include:

```txt
vestige_bootstrap
vestige_search
vestige_expand
vestige_get_project_context
vestige_record_observation
vestige_record_decision
```

### 13.2 Extend `vestige_search`

`vestige_search` should accept a search mode.

Input:

```json
{
  "query": "repo-pinned memory embeddings",
  "mode": "hybrid",
  "limit": 8,
  "depth": "one_liner",
  "include_score_parts": true
}
```

Valid modes:

```txt
lexical
semantic
hybrid
```

If omitted, mode should default to lexical unless project config says otherwise.

### 13.3 MCP Result Shape

Output should remain compact.

Example:

```json
{
  "mode": "hybrid",
  "results": [
    {
      "id": "mem_01",
      "type": "decision",
      "title": "MCP as thin adapter",
      "one_liner": "MCP should wrap Vestige operations without owning storage or lifecycle logic.",
      "score": 0.87,
      "score_parts": {
        "fts": 0.72,
        "vector": 0.91,
        "importance": 0.5,
        "type_boost": 0.1
      },
      "available_depths": ["summary", "compressed", "full", "sources"]
    }
  ]
}
```

### 13.4 MCP Safety

V0.1 should not let agents run embedding or reindex jobs by default.

Embedding operations should remain CLI/admin operations for now.

Optional future MCP admin tools can be added later behind explicit permissions.

## 14. Config Requirements

Add optional embedding configuration to project config.

Example:

```toml
[embeddings]
enabled = false
provider = "fake"
model = "test-small"
dimensions = 64
default_representations = ["summary", "compressed_body"]

[search]
default_mode = "lexical"
```

V0.1 defaults:

```txt
embeddings.enabled = false or unset until user runs embed/configures it
default_mode = lexical
```

If the user runs `vestige embed --all`, Vestige may initialise embedding config if missing.

## 15. Migration Requirements

V0.1 must migrate existing V0 project DBs cleanly.

Acceptance criteria:

```txt
- Existing V0 DB opens successfully after migration.
- Existing memories remain intact.
- Existing FTS search remains intact.
- No embeddings are required after migration.
- New embedding tables are created.
- Running old-style lexical search still works.
```

## 16. Testing Requirements

### 16.1 Unit Tests

Add tests for:

```txt
- embedding provider abstraction
- fake provider determinism
- embedding storage
- stale embedding detection
- embedding job status updates
- vector index insert/query/delete if sqlite-vec is implemented
- hybrid score merge/deduplication
```

### 16.2 CLI Tests

Add tests for:

```txt
- `vestige embed --dry-run`
- `vestige embed --all`
- `vestige embeddings status`
- `vestige search --mode lexical`
- `vestige search --mode semantic`
- `vestige search --mode hybrid`
- missing embeddings behaviour
- deleted memories excluded from semantic/hybrid search
```

### 16.3 MCP Tests

Add tests for:

```txt
- `vestige_search` with lexical mode
- `vestige_search` with semantic mode
- `vestige_search` with hybrid mode
- default mode remains backwards-compatible
- read-only mode remains respected
```

### 16.4 Regression Tests

All existing V0 tests must continue to pass.

## 17. Suggested PR Breakdown

### PR 1 — Embedding Schema and Migrations

Deliverables:

```txt
- memory_embeddings table
- embedding_jobs table
- migration tests
- store methods for embedding metadata
```

Acceptance criteria:

```txt
- existing V0 DB migrates cleanly
- no existing tests break
- embeddings can be absent without affecting recall
```

### PR 2 — Embedding Provider Abstraction

Deliverables:

```txt
- EmbeddingProvider trait/interface
- FakeEmbeddingProvider
- deterministic vectors
- provider/model/dimension metadata
```

Acceptance criteria:

```txt
- tests can generate embeddings without network
- same input produces same vector
- model/dimensions are stored with embeddings
```

### PR 3 — Vector Index Integration

Deliverables:

```txt
- sqlite-vec registration if feasible
- vector table creation
- insert/update/delete vector rows
- nearest-neighbour query
```

Acceptance criteria:

```txt
- vectors are tied to representation IDs
- soft-deleted memories are excluded
- vector data can be rebuilt
```

If sqlite-vec integration blocks progress, create an internal vector search fallback for V0.1 and keep sqlite-vec as V0.1.x.

### PR 4 — CLI Embedding Commands

Deliverables:

```txt
- `vestige embed --all`
- `vestige embed --memory <id>`
- `vestige embed --dry-run`
- `vestige embeddings status`
- `vestige reindex --embeddings`
```

Acceptance criteria:

```txt
- dry-run shows what would be embedded
- failed jobs are visible
- stale/missing counts are visible
```

### PR 5 — Semantic and Hybrid Search

Deliverables:

```txt
- `vestige search --mode semantic`
- `vestige search --mode hybrid`
- hybrid score merging
- result deduplication
- JSON score diagnostics
```

Acceptance criteria:

```txt
- lexical search still works
- semantic search works when embeddings exist
- hybrid search merges lexical and semantic results
- default output remains compact
```

### PR 6 — MCP Search Mode Support

Deliverables:

```txt
- `vestige_search` accepts mode
- `vestige_search` supports score diagnostics
- backwards-compatible defaults
```

Acceptance criteria:

```txt
- MCP smoke tests pass
- hybrid search is available to agents
- read-only mode remains respected
```

### PR 7 — README / Docs Update

Deliverables:

```txt
- V0.1 feature documentation
- embedding setup instructions
- search mode examples
- local-first note about embeddings
- troubleshooting section
```

Acceptance criteria:

```txt
- README shows a minimal embedding workflow
- docs clearly say embeddings are optional/rebuildable
- docs explain lexical vs semantic vs hybrid search
```

## 18. Implementation Notes for Agents

### 18.1 Keep the Slice Narrow

Do not combine V0.1 with dream, federation, daemon, global memory, or automatic ingestion work.

### 18.2 Preserve Backwards Compatibility

Existing commands and MCP tools should keep working.

### 18.3 Avoid Remote Defaults

Do not require OpenAI, Voyage, or any hosted provider for tests or default usage.

### 18.4 Hide Complexity Behind Interfaces

CLI/MCP layers should not know sqlite-vec details.

Preferred layering:

```txt
CLI/MCP
  ↓
core search service
  ↓
store/repository layer
  ↓
FTS index + vector index
```

### 18.5 Treat Embeddings Like Cache

Embedding/index rebuild should be safe.

A broken or missing vector index should not corrupt canonical memory.

## 19. UX Examples

### 19.1 Initial Embedding Flow

```bash
vestige embeddings status
vestige embed --all
vestige embeddings status
```

Expected result:

```txt
Embeddings generated for active memories using summary and compressed_body representations.
```

### 19.2 Semantic Search

```bash
vestige search "why did we avoid a daemon" --mode semantic
```

Expected result:

```txt
mem_12  decision  No daemon required for V0
        V0 should run as CLI + MCP + SQLite before introducing a background service.
```

### 19.3 Hybrid Search

```bash
vestige search "MCP memory search" --mode hybrid
```

Expected result:

```txt
mem_07  decision  MCP search mode support        score 0.89
        Agents should query compact memory cards and expand selected memories by depth.
```

### 19.4 JSON Diagnostics

```bash
vestige search "progressive disclosure" --mode hybrid --json
```

Expected output includes:

```json
{
  "mode": "hybrid",
  "results": [
    {
      "id": "mem_07",
      "score": 0.89,
      "score_parts": {
        "fts": 0.81,
        "vector": 0.86,
        "importance": 0.7,
        "type_boost": 0.05
      }
    }
  ]
}
```

## 20. Definition of Done

V0.1 is complete when:

```txt
- Existing V0 behaviour remains intact.
- Existing tests pass.
- Embedding schema migrates cleanly into existing project DBs.
- Embeddings are optional and rebuildable.
- A deterministic local/test provider exists.
- Embeddings can be generated for active memory representations.
- Embedding status is inspectable.
- Lexical search remains available.
- Semantic search works when embeddings exist.
- Hybrid search merges lexical and semantic results.
- Search results still use progressive disclosure.
- Soft-deleted memories are excluded from semantic/hybrid recall.
- MCP `vestige_search` supports lexical, semantic, and hybrid modes.
- JSON output exposes score diagnostics where practical.
- README/docs include a V0.1 workflow.
```

## 21. Final Scope Statement

Vestige V0.1 adds optional semantic and hybrid recall to the existing repo-pinned memory system.

It introduces representation-level embeddings, an embedding provider abstraction, embedding status/reindex commands, semantic search, hybrid search, and MCP search mode support.

It must preserve the V0 trust model: local-first, project-scoped, inspectable, progressively disclosed, and usable without embeddings.

