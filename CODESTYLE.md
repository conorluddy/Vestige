# Vestige Code Style Guide

> Write for understanding. Optimise for limited attention.
>
> Vestige itself is a memory layer for agents — so the code that builds it should be readable by the same agents we serve. Every token competes for finite attention. Maximise signal, minimise noise.

This guide adapts the general code style to Vestige's stack — Rust 1.80+, a Cargo workspace of small focused crates, SQLite via `rusqlite`, MCP via `rmcp` — and to its product principles (project-pinned scope, progressive disclosure, source-of-truth separation, thin adapters).

## Table of Contents

- [Philosophy](#philosophy)
- [Vestige-Specific Architecture Rules](#vestige-specific-architecture-rules)
- [Progressive Disclosure](#progressive-disclosure)
- [Naming](#naming)
- [Function Design](#function-design)
- [Error Handling](#error-handling)
- [Crate & Module Organisation](#crate--module-organisation)
- [Testing](#testing)
- [Observability](#observability)
- [Agentic Coding Patterns](#agentic-coding-patterns)
- [Project Navigation](#project-navigation)
- [Anti-Patterns](#anti-patterns)
- [Checklist](#checklist)

---

## Philosophy

The optimal code is the minimum necessary to solve the problem correctly. Every additional line is debt.

- **Progressive Disclosure** — Structure code layer-by-layer. Readers grasp high-level flow immediately, drilling into details only when needed. This is also Vestige's product thesis (PRD §5.2): memory is returned as compact handles first, expanded on demand. Our code should mirror that.
- **Self-Documenting** — Names eliminate the need for comments. Comments explain "why," never "what." If you chose algorithm A over B for subtle reasons, state that. If you're working around a `rusqlite` quirk, link the issue.
- **Verbose Naming** — Space occupied by long names is not a problem. Agents will be the primary users of this codebase.
In the same vein as the self-documenting point above, variables, constants, enums, functions and types could all benefit from having verbose yet clear names. Params and nested properties can be more concise. Language is an LLM agents strength, lets take advantage of that. Limit the use of abbreviations. Reject the use of truncated words. 
- **Aggressive Minimalism** — Before adding code, ask: is this the simplest solution? Before adding a comment: does this clarify something non-obvious? Before introducing an abstraction: does this reduce complexity, or relocate it?
- **AHA Over DRY** — Avoid Hasty Abstractions. Wait for the third duplication before extracting. The wrong abstraction is worse than duplication.
- **Source of Truth Separation** — In Vestige terms (PRD §5.3): durable journal vs. derived interpretation vs. disposable indexes. Apply the same discipline in code: keep schema, derived types, and cached views distinct.

## Vestige-Specific Architecture Rules

These rules are non-negotiable for this codebase. They flow from the PRD.

1. **`vestige-core` is the only place business logic lives.** CLI and MCP are thin adapters. If you find yourself writing branching logic in `crates/vestige-cli/src/commands/*` or `crates/vestige-mcp/src/tools/*` beyond argument parsing → call dispatch → output formatting, push it down into core.

2. **MCP exposes intent, not mechanics** (PRD §13.1, §13.9). No raw SQL tools. No destructive defaults. Each tool is a high-level memory operation that maps to a single core function.

3. **Project scope is the default boundary** (PRD §5.1). No code path may search, mutate, or expose memories from a project other than the one resolved from the current `.vestige/config.toml`. Cross-project work waits for federation (V0.7).

4. **Storage layers must stay separable** (PRD §5.3):
   - `memory_events` = durable journal — append-only, never edited.
   - `memories` + `memory_representations` = derived interpretation — replaceable.
   - `memory_fts` and any future vector index = disposable acceleration — rebuildable from the above.

5. **Soft delete only in V0** (PRD §17.1). Never write `DELETE FROM memories`. `vestige forget` flips status; `vestige restore` flips it back.

6. **No daemon, no background threads in V0** (PRD §8). Every CLI invocation opens the SQLite store, does its work, closes. MCP runs for the lifetime of the agent session, but is still single-process and synchronous.

7. **Bytes-not-chars for size limits.** The 2 KiB source-snippet cap (PRD §8 source storage decision) is bytes, truncated at a UTF-8 codepoint boundary. Always.

## Progressive Disclosure

Structure every layer of the system so readers — human or agent — get the right level of detail at the right time. This mirrors how memories disclose: handle → one-liner → summary → compressed → full → sources.

### The Zoom Principle

```text
Level 0 — Workspace layout tells you what exists
crates/
├── vestige-core/    # "the memory engine"
├── vestige-store/   # "SQLite layer"
├── vestige-config/  # ".vestige/ + project identity"
├── vestige-cli/     # "the binary"
└── vestige-mcp/     # "the MCP adapter"

Level 1 — `lib.rs` re-exports tell you the public surface
// crates/vestige-core/src/lib.rs
pub mod error;
pub mod ids;
pub mod representations;
pub mod types;

pub use error::{CoreError, Result};
pub use ids::{MemoryId, ProjectId};
pub use types::{Memory, MemoryStatus, MemoryType, RepresentationDepth, /* ... */};

Level 2 — Function signatures tell you the contract
pub fn record_memory(
    store: &mut Store,
    project_id: &ProjectId,
    input: NewMemory,
) -> Result<Memory>;

Level 3 — Implementation, only read when you need to change behaviour.
```

### File-Level Disclosure

Every file should answer "what is this?" in its first ~10 lines. The order is: module doc → types → public functions → private helpers → tests.

```rust
// ✅ Top of file reveals purpose, types, then public API
//! Deterministic representation derivation (PRD §11.3).
//!
//! Title / one-liner / summary / compressed / full are all derived from a
//! single body string by simple sentence/word slicing. No LLM in V0.

use crate::types::RepresentationDepth;

const MAX_TITLE_CHARS: usize = 60;

pub struct DerivedRepresentations { /* ... */ }

pub fn derive(body: &str) -> DerivedRepresentations { /* ... */ }
pub fn depth_pick<'a>(d: RepresentationDepth, r: &'a DerivedRepresentations) -> &'a str { /* ... */ }

// Private helpers below
fn first_sentence(body: &str) -> &str { /* ... */ }
fn truncate_at_word(s: &str, max_chars: usize) -> String { /* ... */ }

#[cfg(test)]
mod tests { /* ... */ }
```

```rust
// ❌ Implementation soup — must read everything to understand anything
fn helper1() { /* ... */ }
fn helper2() { /* ... */ }
const MAX_TITLE_CHARS: usize = 60;
// 200 lines later...
pub fn derive(body: &str) -> DerivedRepresentations { /* ... */ }
```

### Documentation Disclosure

Match documentation depth to the reader's likely intent.

```text
Level 1 — README.md / CLAUDE.md (5 seconds)
  "Local-first repo-pinned memory for coding agents.
   Entry: `vestige` binary. CLI + MCP over SQLite. No daemon."

Level 2 — Crate-level rustdoc (30 seconds)
  //! `vestige-store`: SQLite-backed persistence. Owns connections,
  //! migrations, FTS5 sync. Higher-level operations live in `vestige-core`.

Level 3 — Section comments (2 minutes)
  // === PUBLIC API ===
  // === MIGRATIONS ===
  // === PRIVATE HELPERS ===

Level 4 — Inline "why" comments (as needed)
  // Re-index FTS rows on restore: rusqlite_migration triggers fired the
  // delete on soft-delete, so the rows are genuinely gone, not orphaned.
```

### Disclosure Anti-Patterns

- **Premature depth** — putting implementation details in `README.md`.
- **Flat disclosure** — 500-line files with no visual hierarchy.
- **Inverted disclosure** — helpers at top, public API buried at the bottom.
- **Missing levels** — jumping from a directory listing straight to inline comments with nothing in between.

## Naming

The single biggest impact on readability. Good names eliminate mental translation.

```rust
// ✅ Descriptive, unambiguous, units in the name where relevant
pub fn truncate_snippet_at_utf8_boundary(input: &str, max_bytes: usize) -> &str { /* ... */ }
pub fn calculate_recency_boost(updated_at: OffsetDateTime, half_life_days: f64) -> f64 { /* ... */ }
pub struct MemoryCounts { pub active: i64, pub deleted: i64 }

// ❌ Vague, abbreviated
pub fn trunc(s: &str, n: usize) -> &str { /* ... */ }
pub fn boost(t: i64, h: f64) -> f64 { /* ... */ }
```

**Rules**

1. **Be specific** — `active_memories` not `memories`, `fts_score` not `score`.
2. **Include units** — `delay_ms`, `max_bytes`, `half_life_days`.
3. **No abbreviations** — `representation` not `repr`, `configuration` not `cfg`. (`CONFIG_DIR` constants are fine.)
4. **Use domain language** — `record_decision`, `forget_memory`, `expand_to_depth`. Match the PRD's vocabulary.
5. **Boolean prefixes** — `is_active`, `has_source`, `can_record`, `should_reindex`.
6. **Verbs for functions** — `derive_representations()` not `representations()`, `resolve_project_id()` not `project_id()`.
7. **Newtype IDs over `String`** — never pass a bare `String` where a `MemoryId` or `ProjectId` belongs.

## Function Design

### Single Responsibility with Explicit Contracts

```rust
// ✅ Self-contained, explicit dependencies, typed contract
pub fn record_decision(
    store: &mut Store,
    project_id: &ProjectId,
    decision: &str,
    rationale: Option<&str>,
    importance: f64,
    source: Option<MemorySource>,
) -> Result<Memory> {
    // All inputs visible in signature. Result encodes every failure mode.
}

// ❌ Hidden dependencies, unclear contract
pub fn record(data: serde_json::Value) -> anyhow::Result<()> {
    // Pulls a global store, infers project from cwd, returns nothing useful.
}
```

### Guard Clauses Over Nesting

Handle edge cases first; keep the happy path unindented.

```rust
// ✅ Guard clauses — happy path clear
pub fn build_context_pack(opts: &ContextOptions) -> Result<ContextPack> {
    if opts.budget_tokens == 0 {
        return Err(CoreError::Validation("budget_tokens must be > 0".into()));
    }
    if opts.include.is_empty() {
        return Err(CoreError::Validation("include must list at least one section".into()));
    }

    let summary = load_summary(opts)?;
    let decisions = load_decisions(opts)?;
    Ok(assemble(summary, decisions, opts))
}

// ❌ Nested — happy path buried
pub fn build_context_pack(opts: &ContextOptions) -> Result<ContextPack> {
    if opts.budget_tokens > 0 {
        if !opts.include.is_empty() {
            // ... happy path 4 levels deep
        }
    }
    Err(/* ... */)
}
```

### Design Rules

1. **Single responsibility** — describable in one sentence.
2. **Explicit dependencies** — pass `&Store`, `&ProjectId`, `&Clock` rather than reading globals.
3. **Type everything** — no `String` where a typed enum, newtype, or branded id will do.
4. **Self-contained context units** — comprehensible without opening other files.
5. **50-line guideline** — refactoring trigger, not a hard cap.
6. **Borrow first, own second** — take `&str` / `&[T]` in signatures unless ownership is genuinely needed.
7. **`#[must_use]` on `Result`-returning APIs** that callers might accidentally drop.

## Error Handling

Vestige uses `Result` everywhere. We do not panic on user-facing paths.

### Layered error strategy

| Layer | Error type | Rationale |
|---|---|---|
| `vestige-core` | `thiserror` enum (`CoreError`) | Domain errors with stable variants — these are the contract. |
| `vestige-store` | `thiserror` enum (`StoreError`) wrapping `rusqlite::Error` | I/O and SQL failures stay typed for retry/log decisions. |
| `vestige-config` | `thiserror` enum (`ConfigError`) | TOML, IO, and identity errors. |
| `vestige-cli` | `anyhow::Result` at the boundary | The CLI just needs to print a useful message and exit non-zero. Use `.context("…")` liberally. |
| `vestige-mcp` | Convert errors into MCP error responses with `code` + `message` + `retryable` | Agents need machine-parseable failures. |

```rust
// ✅ Typed domain error — stable variants
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("memory not found: {0}")] MemoryNotFound(String),
    #[error("invalid representation depth: {0}")] InvalidDepth(String),
    #[error("validation: {0}")] Validation(String),
    // ...
}

// ✅ CLI boundary uses anyhow + context
pub fn run(args: InitArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let store = Store::open(&path).context("opening project store")?;
    Ok(())
}
```

### Newtypes for Validation at Boundaries

Rust's type system is the cheapest way to encode "this string has been validated."

```rust
// ✅ Once you hold a MemoryId, downstream code knows the prefix is valid.
pub struct MemoryId(String);
impl MemoryId {
    pub fn new() -> Self { /* mem_<ULID> */ }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl FromStr for MemoryId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("mem_") { return Err(CoreError::InvalidId(s.into())); }
        Ok(Self(s.into()))
    }
}

// ❌ Untyped — every consumer has to re-validate
pub fn forget_memory(id: String) -> Result<()> { /* ... */ }
```

### Error Principles

1. **Never silently swallow.** No `let _ = …` on `Result`s without a comment explaining why it's safe to ignore.
2. **Fail fast at boundaries.** Validate CLI args and TOML before opening the store. Validate MCP tool inputs before touching SQLite.
3. **Actionable messages.** What failed, what was expected, what to do next.

```rust
// ✅ Actionable
return Err(CoreError::Validation(format!(
    "importance must be in [0.0, 1.0], got {value}. \
     Use --importance 0.5 if unsure."
)));

// ❌ Opaque
return Err(CoreError::Validation("bad input".into()));
```

## Crate & Module Organisation

### Workspace Structure

The workspace splits along context boundaries (PRD §13.1: MCP is a thin adapter over the engine). Crates are small and one-purpose:

```text
Cargo.toml                  # workspace root
crates/
├── vestige-core/           # pure domain — no SQLite, no clap, no MCP
├── vestige-store/          # SQLite + migrations + FTS5
├── vestige-config/         # .vestige/ + project identity
├── vestige-cli/            # bin: `vestige`
└── vestige-mcp/            # MCP server + tool definitions
```

**Dependency direction is strictly downhill:**

```text
cli ──┐
      ├──→ core
mcp ──┤
      └──→ store ──→ core
config ──→ core
```

`vestige-core` depends on no other Vestige crate. Adding an upward dependency (e.g., core needing store) is a code smell — push the integration into the consumer crate instead.

### File-Level Layout

```rust
// ========================================
// === PUBLIC API ===
// ========================================

pub fn record_memory(/* ... */) -> Result<Memory> { /* ... */ }
pub fn forget_memory(/* ... */) -> Result<()> { /* ... */ }

// ========================================
// === REPRESENTATION HANDLING ===
// ========================================

fn persist_representations(/* ... */) -> Result<()> { /* ... */ }

// ========================================
// === PRIVATE HELPERS ===
// ========================================

fn now_rfc3339() -> String { /* ... */ }
```

### Organisation Rules

1. **Group by domain, not by file type.** `crates/vestige-core/src/{memory,search,context,forget}.rs`, not `services/`, `models/`, `repos/`.
2. **One major export per file.** `memory.rs` exports the memory operations. If a file grows past 300 lines, split before it gets worse.
3. **Co-locate tests.** `#[cfg(test)] mod tests { ... }` at the bottom of each file for unit tests. Cross-crate behaviour goes in `crates/<crate>/tests/`.
4. **Public API at the top, helpers at the bottom.** Use `// === SECTION ===` banners when a file has more than one logical group.
5. **`pub use` from `lib.rs`** to flatten the public surface. Consumers should not need to know your internal module tree.
6. **Co-located types.** Each crate has a `types.rs` (or per-feature types alongside the feature). Don't scatter `struct Foo` across five files.

## Testing

### The Trophy

> "Write tests. Not too many. Mostly integration." — Kent C. Dodds.

For Vestige:

1. **Static analysis** (foundation) — `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --check`.
2. **Unit tests** (narrow) — pure functions: representation derivation, ranking math, ID parsing.
3. **Integration tests** (widest — most tests live here) — open a `TempDir`, run real `Store` operations, assert outcomes through the public API.
4. **CLI/MCP smoke** (top) — spawn the built binary against a tmpdir; for MCP, drive the stdio transport in-process. Critical journeys only.

### Test Layout

```text
crates/vestige-core/src/representations.rs
    #[cfg(test)] mod tests { ... }       // unit

crates/vestige-store/tests/
    migrations.rs                         // integration: schema invariants
    fts_triggers.rs                       // integration: FTS sync on soft-delete

crates/vestige-cli/tests/
    init_status.rs                        // spawn `vestige init` against TempDir
```

### Tests as Documentation

Test names describe scenarios. Setup demonstrates intended use.

```rust
#[test]
fn forget_excludes_memory_from_default_search() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    let project = ProjectId::from_slug("vestige");
    store.ensure_project(&project, "Vestige", None, None).unwrap();

    let id = record_memory(&mut store, &project, decision("Use SQLite")).unwrap().id;
    forget_memory(&mut store, &id).unwrap();

    let hits = search(&store, &project, "SQLite", SearchOpts::default()).unwrap();
    assert!(hits.iter().all(|h| h.id != id), "soft-deleted memory leaked into search");
}
```

### Testing Rules

1. **Test behaviour, not implementation.** Drive through the public API; don't peek at private fields.
2. **One concept per test.** A failing test should point at exactly one bug.
3. **Integration over unit** — more confidence per test, more resilient to refactors.
4. **Deterministic.** Use a fixed `Clock`/seed for anything time- or randomness-dependent.
5. **Prefer `TempDir` over mocks.** Real SQLite in a tmpdir is fast and exercises the schema.
6. **Cover the soft-delete + FTS interaction.** It's the most subtle piece of M0–M3.

## Observability

### Structured Logging via `tracing`

```rust
// ✅ Structured — queryable, correlated
tracing::info!(
    project_id = %project_id,
    memory_id = %memory.id,
    type = %memory.r#type,
    duration_ms = elapsed.as_millis() as u64,
    "memory recorded"
);

// ❌ Stringly typed
tracing::info!("Recorded memory {} for {}", memory.id, project_id);
```

### What to Log

- **Always include where relevant** — `project_id`, `memory_id`, `operation`, `duration_ms`, error variant name.
- **Critical boundaries** — DB open, migration runs, FTS index changes, MCP tool entry/exit, every `forget`/`restore`.
- **Errors with context** — log at the boundary that converts a typed error into an `anyhow::Error`, including the variant.

### Log Configuration

The CLI honours `VESTIGE_LOG=<env-filter>` (default `warn`). Logs go to stderr only — stdout is reserved for command output so `--json` consumers can parse it cleanly.

## Agentic Coding Patterns

These patterns matter doubly here: Vestige's primary user is an agent, and Vestige itself is built collaboratively with agents.

### Idempotent Operations

```rust
// ✅ Idempotent — `vestige init` can run any number of times safely
pub fn ensure_project(
    store: &mut Store,
    id: &ProjectId,
    name: &str,
    /* ... */
) -> Result<ProjectRecord> {
    // INSERT … ON CONFLICT(id) DO UPDATE … — never errors on re-run.
}

// ❌ Non-idempotent — explodes the second time `init` runs
pub fn create_project(store: &mut Store, id: &ProjectId, name: &str) -> Result<ProjectRecord> {
    // raw INSERT — UNIQUE constraint failure on re-init.
}
```

Every mutation Vestige exposes — `init`, `remember`, `forget`, `restore` — has an obvious idempotent shape. Use `ensure_*` naming when the operation is "make this true."

### Explicit State Machines

Memory status is a discriminated set, not a free-form string.

```rust
// ✅ Exhaustive enum — compiler catches new states
pub enum MemoryStatus { Active, Deleted /* later: Pinned, Archived, Superseded, Contradicted */ }

// ❌ Stringly typed — silent typos, no exhaustiveness checks
let status: String = row.get(0)?;
if status == "actve" { /* never matches; bug ships */ }
```

Same for `MemoryType` and `RepresentationDepth`. New variants should be deliberate edits to the enum, not new magic strings.

### Machine-Parseable Errors at the MCP Boundary

```rust
// ✅ Agents reason about codes, humans read the message.
pub struct ToolError {
    pub code: &'static str,        // "PROJECT_NOT_FOUND", "INVALID_DEPTH"
    pub message: String,
    pub retryable: bool,
}
```

CLI text output is for humans; MCP responses must be structured.

### Atomic, Independently-Verifiable Changes

Each milestone (M0 → M5) is its own commit/PR. Within a milestone, group related changes. Splitting M0 across "schema PR" and "command PR" makes review tractable; bundling M0 + M3 makes it a slog.

### Convention Over Configuration

- **Bin name** is `vestige`, the workspace name is `vestige`, the package directory is `vestige-cli`. Don't invent new shorthand.
- **ID prefixes** are fixed: `mem_<ULID>`, `proj_<slug-or-hash>`, `evt_<ULID>`.
- **Path constants** live in `vestige-config` (`CONFIG_DIR`, `CONFIG_FILE`). Never hardcode `.vestige` in another crate.
- **Migrations** are numbered SQL files: `000N_<short_description>.sql`. Never edit a shipped migration; add a new one.
- **CLI commands** map 1:1 to a file under `crates/vestige-cli/src/commands/`. The dispatcher in `main.rs` is mechanical.

### Contract-First Design

Define types and trait shapes before bodies. Vestige's V0 contract is the PRD §11.5 schema and the §13.2–§13.8 MCP tool I/O. Code those before writing logic.

```rust
// 1. The contract
pub trait MemoryEngine {
    fn record(&mut self, project: &ProjectId, input: NewMemory) -> Result<Memory>;
    fn forget(&mut self, id: &MemoryId) -> Result<()>;
    fn search(&self, project: &ProjectId, q: &str, opts: SearchOpts) -> Result<Vec<MemoryCard>>;
    fn expand(&self, id: &MemoryId, depth: RepresentationDepth) -> Result<String>;
}

// 2. Then implement.
```

### Observable Side Effects

Every mutation appends a `memory_events` row (PRD §11.5). That table is the durable journal — don't bypass it for "performance" or "convenience." If a mutation isn't worth journalling, it probably isn't worth doing.

### Context Optimisation & Token Economics

Vestige exists to reduce agent token spend. Hold the same standard for the code we ship.

#### Semantic Compression in MCP

```text
❌ Granular — N tools, ~Nk tokens of schema
[ vestige_search_decisions, vestige_search_notes, vestige_search_questions,
  vestige_search_observations, vestige_search_preferences, ... ]

✅ Semantic — one tool, intent in args
{
  name: "vestige_search",
  parameters: {
    query: "string",
    types: "decision | note | observation | …  (optional)",
    depth: "one_liner | summary | compressed | full",
    limit: "number"
  }
}
```

The PRD's `vestige_search` / `vestige_expand` / `vestige_get_project_context` split is already the semantic-compression shape. Don't fragment it.

#### Layered Context Loading

Search returns one-liner cards by default (PRD §12.5 / §13.4). Full bodies require an explicit `expand` call. This is the product principle — apply it to your own internal APIs too:

```rust
// ✅ Summary first, drill-down on demand
pub struct MemoryCard {
    pub id: MemoryId,
    pub r#type: MemoryType,
    pub title: String,
    pub one_liner: String,
    pub score: f64,
    pub available_depths: Vec<RepresentationDepth>,
}

// ❌ Eager — every search hit drags the full body and source content
pub struct MemoryFull {
    pub id: MemoryId,
    pub all_representations: HashMap<RepresentationDepth, String>,
    pub sources: Vec<MemorySource>,
    /* ... */
}
```

#### Token-Aware Documentation

```markdown
# ❌ Token-heavy
The Vestige memory engine is responsible for handling all aspects of memory
management within the Vestige application. It was designed with a number of
best practices in mind and provides a comprehensive interface for…

# ✅ Token-efficient
## vestige-core
Project-scoped memory engine. SQLite-backed via `vestige-store`.
- Entry: `MemoryEngine` trait
- IDs: `mem_<ULID>`, `proj_<slug>` (newtypes in `ids.rs`)
- Errors: `CoreError` (thiserror)
```

#### Structured Output for `--json`

Every CLI command that prints results must support `--json` with a stable schema. Agents parsing CLI output should never need a regex.

```rust
// ✅ Discriminated, scannable
#[derive(serde::Serialize)]
struct SearchOutput {
    query: String,
    results: Vec<MemoryCardJson>,
}

// ❌ Stringly formatted
println!("Found {} results for '{}':\n{}", n, query, formatted);
```

#### Context Boundaries as Architecture

`vestige-core` does not import `clap`, `rmcp`, or `rusqlite` types. `vestige-mcp` does not import `clap`. Each crate is loadable as a coherent context unit on its own.

#### Compression Strategies Reference

| Strategy | Before | After | Savings |
|---|---|---|---|
| Semantic MCP tools | 1 tool per memory type | 1 `vestige_search` with `types` arg | (N-1) × tool schema |
| Layered loading | Full body in every search hit | One-liner card + `expand` on demand | 70–95% per search |
| Dense docs | Narrative crate-level docs | Triple-bullet rustdoc | ~50% |
| Co-located types | `types.rs` per crate | Single import to understand a domain | Fewer file loads |
| Discriminated unions | Generic `Error` + message | `CoreError` enum + `code` at MCP edge | Eliminates parsing |

## Project Navigation

### Root-Level `CLAUDE.md`

Keep it under 200 lines. Cover entry points, common tasks, and the build/test commands.

```markdown
# Vestige

Local-first repo-pinned memory layer for coding agents. CLI + MCP over SQLite. No daemon.

## Entry points
- Binary: `vestige` (crates/vestige-cli)
- Library API: `vestige-core` (memory engine), `vestige-store` (SQLite), `vestige-config` (.vestige/)
- MCP: `vestige mcp` → `vestige-mcp::run`

## Workspace
- Run: `cargo run -p vestige -- <command>` (package is `vestige`; directory is `crates/vestige-cli/`)
- Test: `cargo test`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format: `cargo fmt`

## Common tasks
- Add a CLI command → file under `crates/vestige-cli/src/commands/`, register in `main.rs`.
- Add an MCP tool → file under `crates/vestige-mcp/src/tools/`, expose from `lib.rs`.
- Schema change → new numbered migration under `crates/vestige-store/src/migrations/`. Never edit shipped migrations.
- New memory type → add variant to `MemoryType` in `vestige-core::types`.
```

### Crate-Level `README.md`

Each crate's README answers: what is this, what's its public API, what are the gotchas?

```markdown
# vestige-store

SQLite-backed persistence. Owns connections, migrations, and the FTS5 sync triggers.

## Public API
- `Store::open(path) -> Result<Store>`
- `Store::ensure_project(...)`, `Store::memory_counts(...)`, `Store::record_event(...)`

## Gotchas
- Migrations are immutable once shipped — bump and add, never edit.
- `journal_mode = WAL` is set on every open; don't override.
- FTS rows are deleted on soft-delete via trigger and re-inserted on restore.
```

### Progressive Context Hierarchy

1. **`CLAUDE.md` / root `README.md`** — system overview, entry points, build commands.
2. **Crate `README.md`** — purpose, public API, gotchas.
3. **`//!` crate-level rustdoc** — module purpose, key invariants.
4. **Section comments in files** — `// === PUBLIC API ===`.
5. **`///` item-level rustdoc** — public functions and types.
6. **Inline `// why` comments** — only for non-obvious decisions.

## Anti-Patterns

- **Premature optimisation** — measure first. SQLite is fast.
- **Hasty abstractions** — wait for the third duplication.
- **Clever code** — simple and obvious beats compact and clever.
- **Silent failures** — never `let _ = result;` without a comment.
- **Vague interfaces** — `fn handle(input: serde_json::Value) -> anyhow::Result<serde_json::Value>` provides no contract. Use typed inputs.
- **Hidden dependencies** — `lazy_static` stores, ambient `Connection` singletons, reading env vars deep in core.
- **Nested conditionals** — use guard clauses.
- **Comments describing "what"** — if you need a comment to explain what code does, rename things.
- **Premature generalisation** — V0 first. The PRD's deferred items are deferred for a reason.
- **Token bloat** — full memory bodies in search responses; verbose log lines.
- **Inverted disclosure** — helpers at top, public API buried.
- **Flat files** — 500 lines with no section banners.
- **Leaky context boundaries** — `vestige-core` reaching for `rusqlite::Row`. Push it back to `vestige-store`.
- **Eager context loading** — MCP tools that return full bodies + sources by default.
- **Editing a shipped migration** — always add a new one. Old DBs in `~/.vestige/projects/*/` won't re-run a mutated migration.
- **Hard delete** — V0 forbids it. `forget` is soft.
- **Cross-project queries** — V0 forbids them. Federation lives in V0.7.
- **Synchronous I/O during MCP request handling that blocks for seconds** — keep operations bounded; large work belongs to a future daemon (V0.4), not V0 MCP.

## Checklist

Before opening a PR:

- [ ] Solves the stated problem with minimal code.
- [ ] A reader new to the file can understand it without opening adjacent files.
- [ ] Errors return typed variants with actionable messages; no `unwrap`/`panic` on user-facing paths.
- [ ] Names are specific, units explicit, no abbreviations.
- [ ] Functions have a single, sentence-describable responsibility.
- [ ] Dependencies are explicit parameters, not globals.
- [ ] Newtype IDs (`MemoryId`, `ProjectId`) used everywhere — no bare `String`.
- [ ] `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all clean.
- [ ] Mutations are idempotent (or explicitly named `create_*` if not).
- [ ] Soft-delete invariant holds — no `DELETE FROM memories`.
- [ ] CLI command supports `--json` if it prints results.
- [ ] MCP tool returns structured errors (`code`, `message`, `retryable`).
- [ ] No `vestige-core` imports of `clap` / `rusqlite` / `rmcp`.
- [ ] No cross-project access.
- [ ] New schema → new numbered migration; no edits to shipped ones.
- [ ] Public API scannable at the top of each file (types → functions → helpers → tests).
- [ ] Documentation is dense — no filler prose.

---

> *"Any fool can write code that a computer can understand. Good programmers write code that humans can understand."* — Martin Fowler
>
> Vestige adds a corollary: good code is also code that an agent can reason about with minimum context. Build for both.
