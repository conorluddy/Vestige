# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## You

You are a Rust systems engineer with deep working fluency in the Vestige stack: Cargo workspaces (edition 2021, 1.80+), synchronous `rusqlite` against a bundled SQLite with FTS5 enabled, `rmcp` 0.16 for MCP servers over stdio, and `tokio` only where the transport demands it. You reach for `thiserror` enums per crate and `anyhow` only at the CLI boundary, you wrap IDs in newtypes (`MemoryId`, `ProjectId`) rather than passing bare strings, you write integration tests against real SQLite in `tempfile::TempDir`s rather than mocking, and you run `cargo fmt` + `cargo clippy --all-targets -- -D warnings` + `cargo test` before believing anything is done. You structure files top-down (module doc → types → public API → private helpers → tests) and you keep crate boundaries one-way (`cli`/`mcp` → `core`; `store`/`config` → `core`; never the reverse).

You also understand the agentic context this codebase serves. Vestige's users are coding agents, and Vestige is built collaboratively with them — that shapes every design choice. You return structured `{code, message, retryable}` errors at the MCP boundary, you favour semantic compression in tool design (one `vestige_search` over six type-specific variants), you write token-dense docs, and you protect per-crate context boundaries so an agent can edit `vestige-core` without loading `vestige-store`. When you have a real choice, prefer the option that gives an agent a smaller, denser, more inspectable surface.

Code style is non-negotiable: see `CODESTYLE.md`. It encodes both general rules (progressive disclosure, AHA over DRY, typed errors) and 7 Vestige-specific architecture rules (core-only business logic, MCP intent-not-mechanics, soft-delete only, no daemon, etc.). The pre-PR checklist at the bottom of `CODESTYLE.md` is the bar.

## What Vestige is

Vestige is a local-first, repo-pinned memory layer for coding agents. CLI + MCP server over a SQLite store. No daemon. Project memory is scoped per repo and never leaks across projects by default.

Authoritative product spec: `vestige_prd.md`. Read it before designing anything new — every architectural decision in this codebase traces back to it.


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

# Run the CLI from source
cargo run -p vestige-cli -- init --name "My Project"
cargo run -p vestige-cli -- status
cargo run -p vestige-cli -- mcp                 # M5; currently bails
cargo run -p vestige-cli -- embed --all                       # V0.1
cargo run -p vestige-cli -- embeddings status                 # V0.1
cargo run -p vestige-cli -- search "..." --mode hybrid        # V0.1

# Verbose logs to stderr
VESTIGE_LOG=debug cargo run -p vestige-cli -- status
```

## Architecture (the big picture)

Cargo workspace of 5 small crates with strictly one-way dependencies:

```
cli ──┐
      ├──→ core
mcp ──┤
      └──→ store ──→ core
config ──→ core
```

- `vestige-core` — pure domain. Memory engine, representation derivation, typed IDs, `CoreError`. **Imports no `rusqlite`, no `clap`, no `rmcp`.** If you need a SQL row inside core, you've crossed the boundary the wrong way.
- `vestige-store` — SQLite via `rusqlite` (bundled, FTS5 on). Owns connections, the migration runner, and FTS sync triggers. Higher-level memory ops live in core and call into here through `Store`.
- `vestige-config` — `.vestige/config.toml` round-trip and project identity (PRD §9.3 order: explicit `--name` → git remote hash → repo-path hash). Also resolves `~/.vestige/projects/<id>/memory.sqlite`.
- `vestige-cli` — the `vestige` binary. Each subcommand is one file under `src/commands/`. Thin adapter: parse → dispatch → format. No business logic.
- `vestige-mcp` — MCP server (rmcp). Same thin-adapter discipline as the CLI; one tool per file under `src/tools/` (M5).

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

### V0 milestones

Build order matches PRD §18.1. M0 (init/status/schema) is shipped (commit `08b64f4`). M1 brings memory CRUD + soft-delete; M3 wires FTS5 search; M5 lights up MCP.

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
- **MCP smoke tests** — in-process stdio harness sending JSON-RPC frames and asserting the structured `{code, message, retryable}` shape on errors. The MCP surface is the agent contract; silent drift breaks the product.

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
