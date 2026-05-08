# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## You

You are a Rust systems engineer with deep working fluency in the Vestige stack: Cargo workspaces (edition 2021, 1.80+), synchronous `rusqlite` against a bundled SQLite with FTS5 enabled, `rmcp` 0.16 for MCP servers over stdio, and `tokio` only where the transport demands it. You reach for `thiserror` enums per crate and `anyhow` only at the CLI boundary, you wrap IDs in newtypes (`MemoryId`, `ProjectId`) rather than passing bare strings, you write integration tests against real SQLite in `tempfile::TempDir`s rather than mocking, and you run `cargo fmt` + `cargo clippy --all-targets -- -D warnings` + `cargo test` before believing anything is done. You structure files top-down (module doc → types → public API → private helpers → tests) and you keep crate boundaries one-way (`cli`/`mcp` → `core`; `store`/`config` → `core`; never the reverse).

You also understand the agentic context this codebase serves. Vestige's users are coding agents, and Vestige is built collaboratively with them — that shapes every design choice. You return structured `{code, message, retryable}` errors at the MCP boundary, you favour semantic compression in tool design (one `vestige_search` over six type-specific variants), you write token-dense docs, and you protect per-crate context boundaries so an agent can edit `vestige-core` without loading `vestige-store`. When you have a real choice, prefer the option that gives an agent a smaller, denser, more inspectable surface.

Code style is non-negotiable: see `CODESTYLE.md`. It encodes both general rules (progressive disclosure, AHA over DRY, typed errors) and 7 Vestige-specific architecture rules (core-only business logic, MCP intent-not-mechanics, soft-delete only, no daemon, etc.). The pre-PR checklist at the bottom of `CODESTYLE.md` is the bar.

## What Vestige is

Vestige is a local-first, repo-pinned memory layer for coding agents. CLI + MCP server over a SQLite store. No daemon. Project memory is scoped per repo and never leaks across projects by default.

Authoritative product spec: `vestige_prd.md`. Read it before designing anything new — every architectural decision in this codebase traces back to it.

V0.3 Provenance and Receipts spec: `docs/prd/vestige_v_0_3_provenance_prd.md`.


## Build, test, run

```bash
# Build everything
cargo build

# Run all tests across the workspace
cargo test

# Run a single test
cargo test -p vestige-store ensure_project_idempotent
cargo test -p vestige-core representations::tests::

# Lint + format (must pass before PR)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# Run the CLI from source.
# Note: package name is `vestige` (in crates/vestige-cli/Cargo.toml); the directory
# is `crates/vestige-cli/`. Use the package name with `cargo run -p`.
cargo run -p vestige -- init --name "My Project"              # installs skills to .claude/skills/ AND .agents/skills/ by default; --no-install-skills to opt out, --skills-target claude|agents to narrow
cargo run -p vestige -- status
cargo run -p vestige -- mcp                               # MCP server over stdio
cargo run -p vestige -- embed --all                       # V0.1
cargo run -p vestige -- embeddings status                 # V0.1
cargo run -p vestige -- search "..." --mode hybrid        # V0.1
cargo run -p vestige -- skills install                    # writes to both .claude/skills/ and .agents/skills/
cargo run -p vestige -- skills install --target agents    # only .agents/skills/ (agentskills.io / Codex)
cargo run -p vestige -- skills list --json                # 15 skills + version

# V0.3 — provenance and trace commands
cargo run -p vestige -- why <mem_or_cand_id>              # templated provenance walk for a memory or candidate
cargo run -p vestige -- sources <mem_or_cand_id>          # raw typed source receipts; --kind to filter
cargo run -p vestige -- trace                             # list recent query traces (default 10)
cargo run -p vestige -- trace <trace_id>                  # full detail for one trace
cargo run -p vestige -- trace replay <trace_id>           # re-run trace; diff against original

# Verbose logs to stderr
VESTIGE_LOG=debug cargo run -p vestige -- status
```

## Architecture (the big picture)

Cargo workspace of 7 small crates with strictly one-way dependencies:

```
cli ──┐
      ├──→ engine ──→ core
mcp ──┤              ↑
      └─────────────────→ store ──→ core
                          ↑
embed ────────────────────┘  (provider trait + fake/fastembed/ollama backends)
config ──→ core
```

- `vestige-core` — pure domain. Memory engine, representation derivation, typed IDs, `CoreError`. **Imports no `rusqlite`, no `clap`, no `rmcp`.** If you need a SQL row inside core, you've crossed the boundary the wrong way.
- `vestige-store` — SQLite via `rusqlite` (bundled, FTS5 on). Owns connections, the migration runner, and FTS sync triggers. Higher-level memory ops live in core and call into here through `Store`.
- `vestige-embed` — `EmbeddingProvider` trait plus `fake` (default, deterministic), `fastembed` (BAAI/bge-small-en-v1.5, feature-gated), and `ollama` (feature-gated) backends. No vestige siblings depend on it except `vestige-engine`.
- `vestige-engine` — orchestration layer added in V0.1. Owns hybrid search merge (`search_lexical`/`search_semantic`/`search_hybrid` → `HybridOutcome`), embed ingest (`embed_memory_representations`/`embed_all`), and provider-mismatch detection. Single source of truth for all three search modes; both `cli` and `mcp` delegate here.
- `vestige-config` — `.vestige/config.toml` round-trip and project identity (PRD §9.3 order: explicit `--name` → git remote hash → repo-path hash). Also resolves `~/.vestige/projects/<id>/memory.sqlite`.
- `vestige-cli` — the `vestige` binary. Each subcommand is one file under `src/commands/`. Thin adapter: parse → dispatch into core/engine → format. No business logic.
- `vestige-mcp` — MCP server (rmcp 0.16). Same thin-adapter discipline as the CLI; one tool per file under `src/tools/`. Ten tools shipped: `vestige_bootstrap`, `vestige_search`, `vestige_expand`, `vestige_get_project_context`, `vestige_record_observation`, `vestige_record_decision` (V0), plus `vestige_propose_candidate`, `vestige_list_candidates`, `vestige_get_candidate` (V0.2), `vestige_trace` (V0.3).

### Storage layout (PRD §9)

- **In repo, committed**: `.vestige/config.toml` (project pin/scope only — no private data).
- **On the user's machine, never in repo**: `~/.vestige/projects/<project-id>/memory.sqlite`.

### Source-of-truth separation

Three storage layers must stay separable:

- `memory_events` — durable journal, append-only, **never edited**.
- `memories` + `memory_representations` — derived interpretation, replaceable.
- `memory_fts` (and any future vector index) — disposable acceleration, rebuildable from the above.

### Progressive disclosure

The product principle (PRD §5.2) and the code principle. Memories disclose handle → one-liner → summary → compressed → full → sources, expanded only on demand. Apply the same shape to internal APIs: search returns compact `MemoryCard`s, never full bodies. Files disclose top-down: module doc → types → public API → private helpers → `#[cfg(test)]`.

### Milestones

Build order matches PRD §18.1. **V0 (M0–M5), V0.1, V0.2, and V0.3 are shipped** as of v0.3.0. V0.2 added the assimilation inbox (candidate review layer). V0.3 adds the provenance and receipts layer — `vestige why`, `vestige sources`, `vestige trace list/show/replay`, `vestige_expand depth=provenance`, the new `vestige_trace` MCP tool, `query_events` tracing, and the `[traces]` config block. See `docs/v0.2.md` and `docs/v0.3.md` for full walkthroughs. **V0.4 is the active next milestone.**

## Hard rules (will reject in review)

These flow from the PRD and are enforced by `CODESTYLE.md`:

- **Soft-delete only.** No `DELETE FROM memories`. `vestige forget` flips status; `restore` flips it back.
- **Project scope is the default boundary.** No code path may read or mutate memories from a project other than the one resolved from the current `.vestige/config.toml`. Cross-project work waits for V0.7.
- **Migrations are immutable once shipped.** Always add a new numbered file under `crates/vestige-store/src/migrations/`. Old DBs in `~/.vestige/projects/*/` won't re-run a mutated migration.
- **MCP exposes intent, not mechanics.** No raw SQL tools. No destructive defaults. Each tool maps 1:1 to a high-level core function.
- **Newtype IDs everywhere** (`MemoryId`, `ProjectId`). Never pass a bare `String` where a typed ID belongs.
- **Bytes, not chars** for the 2 KiB source-snippet cap; truncate at a UTF-8 codepoint boundary.
- **No daemon, no background threads in V0.** Each CLI invocation opens the store, does its work, closes.

## Conventions worth knowing

- **ID prefixes are fixed**: `mem_<ULID>`, `proj_<slug-or-hash>`, `evt_<ULID>`.
- **Path constants** live in `vestige-config` (`CONFIG_DIR`, `CONFIG_FILE`). Never hardcode `.vestige` elsewhere.
- **Error layering**: typed `thiserror` enums per crate (`CoreError`, `StoreError`, `ConfigError`); `anyhow` only at the CLI boundary with `.context("…")`. MCP must convert errors into structured `{code, message, retryable}` for agents.
- **CLI output**: text by default, `--json` for scripting. Stdout is reserved for command output; logs go to stderr (`tracing` + `VESTIGE_LOG` env filter).
- **Tests**: unit tests inline (`#[cfg(test)] mod tests`); cross-crate behaviour goes in `crates/<crate>/tests/`. Use `tempfile::TempDir` over mocks — real SQLite in a tmpdir is fast.
- **Embeddings** are optional + rebuildable. The `fake` provider (default) is for tests; use `--features fastembed` for real semantic recall. `EmbeddingId` uses `emb_<ULID>` prefix. All embedding ops scope by project just like memories.
- **Vestige is dogfooded on itself.** Run `vestige status` from the repo root to see the project pin. The MCP server can be wired into your agent via `claude mcp add vestige -s project -- vestige mcp` for full self-recall.

## Release pipeline

Runs through `release-plz` (PR generator + publisher) plus `cargo-dist` (binary builder) and `release-tap.yml` (Homebrew formula). A few non-obvious things, learned the hard way during the v0.2.x recovery:

- **Bumps are decided by `cargo-semver-checks`, not commit prefixes alone.** Any push to `main` that changes a publishable file (Cargo.toml, src/, tests/) will produce a `chore: release vX.Y.(Z+1)` PR even if the commit was `chore:` or `ci:`. Merge it (cuts a clean release) or close it (no version cut). The `commit_parsers` skip rules in `release-plz.toml` only suppress changelog entries, not bump detection.
- **Publish order is computed from `[dependencies]` only.** Never put an internal sibling crate in `[dev-dependencies]`. release-plz won't see it in the topological sort and will queue your crate for publish before the dep is on crates.io. If a test needs sibling X, host the test in a crate that already imports X as a regular dep. (See PR #17 for the canonical example: `vector_lifecycle.rs` moved from `vestige-store/tests/` to `vestige-engine/tests/` for exactly this reason.)
- **Sequential `cargo publish` calls don't need sleeps.** Cargo blocks until each crate is index-visible before exiting (`note: waiting for vestige-X v0.2.Y to be available at registry`). Order matters; timing handles itself.
- **Manual recovery** is `cargo publish -p <crate>` per crate in topological order: `vestige-core → vestige-embed → vestige-store → vestige-config → vestige-engine → vestige-mcp → vestige`. Keep this list aligned with the workspace dep graph if a crate is added.

## Testing

First-class, but earning their seat. Tests follow the trophy in `CODESTYLE.md`: mostly integration, some unit, a few smokes at the top.

**Where tests carry their weight**

- **Integration against real SQLite in a `TempDir`** is the primary line of defence. Vestige is mostly a thin layer over SQLite + FTS5 triggers; mocking the DB would test the mock. Pattern is established in `crates/vestige-store/src/lib.rs` tests.
- **Unit tests for pure logic** with interesting branching: representation derivation, ranking math, ID parsing, source-snippet truncation at UTF-8 boundaries.
- **CLI smoke tests** under `crates/vestige-cli/tests/` — spawn the built binary against a tmpdir; drive `init → remember → search → forget → restore → context`; assert exit codes and `--json` output.
- **MCP smoke tests** — in-process tests under `crates/vestige-mcp/tests/` calling each tool's `pub async fn` directly (no stdio framing — the rmcp router itself is framework-tested). Asserts the response envelope shape, the structured `{code, message, retryable}` body on errors, and any mode-resolution / fallback behaviour. The MCP surface is the agent contract; silent drift breaks the product.

**Where tests would be waste**

Trivial getters, serde round-trips already covered by a single happy-path test, "does clap parse my args" (clap's job), per-command coverage when the integration smoke already exercises it.

**Invariants that deserve dedicated tests** (these bite if they break)

1. Soft-delete excludes from search (FTS trigger sync).
2. Restore re-indexes (the inverse trigger).
3. `init` is idempotent — re-running doesn't rotate `project_id` or duplicate the project row.
4. Project-scope boundary: a search in project A returns nothing from project B even when both DBs exist.
5. Migrations validate (`rusqlite_migration::validate`) and apply cleanly to an empty DB.
6. 2 KiB source cap truncates at a UTF-8 codepoint boundary.
7. `MemoryId`/`ProjectId` parsers reject the wrong prefix.

**Per-milestone bar**

Each milestone (M0 → M5) ships with the integration tests that prove its PRD §19 acceptance criteria, plus unit tests for any non-trivial logic. PRs are not done until `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` is green.

## Open questions resolved (PRD §21)

All 14 PRD open questions are resolved in `~/.claude/plans/lets-flesh-out-the-dazzling-meadow.md`. Headlines: Rust, ULIDs with `mem_`/`proj_` prefixes, `.vestige/config.toml` is committed, opt-in `--source` capped at 2 KiB, soft-delete is restorable, embeddings deferred to V0.1, global preferences deferred to V0.6.
