# Vestige

> Local-first, repo-pinned memory for coding agents.
>
> Landing page: <https://conorluddy.github.io/Vestige/>

> **vestige** *noun*  ˈve-stij
>
> 1. *(a)* a trace, mark, or visible sign left by something (such as an ancient city or a condition or practice) vanished or lost; the smallest quantity or trace.
> 2. *(b)* a footprint.
>
> — *Merriam-Webster*

Modern coding agents lose useful context between sessions. They re-discover the same project decisions, naming conventions, architecture constraints, and open questions every time you start a new conversation. Most "memory" approaches collapse everything into a global vector soup — stale recall, context pollution, no trust.

Vestige fixes this by giving each repo its own durable, inspectable memory store, exposed to agents through MCP and to humans through a CLI.

```text
A repo can leave useful traces.
An agent can recall those traces later.
A human can inspect and control them.
```

## What Vestige is (and isn't)

**Vestige is** a small Rust binary (`vestige`) plus a SQLite memory store. Each repo gets its own scope. Memories disclose progressively — agents pull compact one-liners first, expand on demand. There's no daemon, no cloud, no automatic ingestion.

**Vestige isn't** a chatbot, a note-taking app, a vector database, or an agent framework. It's the memory layer you wire your agent into.

## Install (current state)

There's no Homebrew formula or crates.io release yet — install from source.

**Prerequisites**

- Rust 1.80+ (`rustup update stable` if needed)
- A C toolchain (Xcode CLI tools on macOS, `build-essential` on Debian) — `rusqlite` bundles SQLite and needs to compile it once.

**Install from crates.io**

```bash
cargo install vestige
```

**Or via Homebrew** (compiles from source via the `rust` formula):

```bash
brew tap conorluddy/vestige
brew install vestige
```

**Or build from source**

```bash
git clone https://github.com/conorluddy/Vestige.git
cd Vestige
cargo install --path crates/vestige-cli
```

Any route puts `vestige` in `~/.cargo/bin/` (or `$(brew --prefix)/bin/`). Make sure that directory is on your `PATH`.

**Sanity check**

```bash
vestige --version
vestige --help
```

To upgrade later: `cargo install vestige` (cargo overwrites the existing binary), `brew upgrade vestige`, or `git pull && cargo install --path crates/vestige-cli` if you're tracking source.

## Try it (CLI demo)

```bash
cd ~/code/my-project
vestige init --name "My Project" --summary "An app for tracking useful things."

vestige decision add "Use SQLite as the canonical local store." \
  --rationale "Durability and portability beat a hosted DB for V0."
vestige note add     "MCP should be a thin adapter over the memory engine."
vestige question add "Should embeddings ship in V0.1 or V0?"

vestige status                              # shows project + DB path
vestige search "architecture"               # one-liner cards, ranked
vestige list --type decision --json         # machine-readable
vestige show mem_01HXXXXXXXXXXXXXXXXXX --depth full
vestige context --budget-tokens 1200        # the full project pack
```

Soft-delete and restore are first-class:

```bash
vestige forget   mem_01HXXXXXXXXXXXXXXXXXX
vestige restore  mem_01HXXXXXXXXXXXXXXXXXX
```

Every command supports `--json` for scripting. `VESTIGE_LOG=debug` turns on structured stderr logs.

## Semantic recall (V0.1)

V0 ships with BM25 lexical search. V0.1 adds embeddings and hybrid recall, so agents can find memories that don't share keywords with the query.

```bash
# One-time: embed all active memories with the default provider (fake; deterministic, no model download).
vestige embed --all
vestige embeddings status

# Search modes:
vestige search "why did we pick our store" --mode semantic
vestige search "agent memory layer" --mode hybrid       # merges FTS + vector
vestige search "agent memory layer" --hybrid --json    # convenience alias + score breakdown
```

### Real semantic quality (recommended)

The default `fake` provider is deterministic and exists for tests — it doesn't produce semantically meaningful vectors. For real recall, build with the `fastembed` feature, which downloads BAAI/bge-small-en-v1.5 (~60MB, cached at `~/.vestige/models/`) on first use:

```bash
cargo install vestige --features fastembed
# Then in your project's .vestige/config.toml:
[embeddings]
provider = "fastembed"
```

Or use Ollama (build with `--features ollama`):

```toml
[embeddings]
provider = "ollama"
model = "nomic-embed-text"
```

### Embeddings are optional and rebuildable

Memories are canonical in SQLite. Embeddings are an index — delete `.vestige` and reseed, or just `vestige reindex --embeddings` to recompute. The lexical fallback always works, even with no embeddings. Hybrid mode falls back to lexical with a warning when embeddings are missing.

### Scaling note

V0.1 uses a brute-force cosine scan over a `BLOB` column (no `vec0` virtual table yet). It's comfortable up to roughly 10k vectors per project; past that, semantic-mode latency starts to show. A future release will swap in an indexed implementation behind the same `Store` API.

## Plug it into Claude Code (MCP)

Vestige speaks MCP over stdio. From inside a repo where you've already run `vestige init`:

```bash
# Add Vestige as an MCP server, scoped to this project:
claude mcp add vestige -s project -- vestige mcp
```

`-s project` writes the entry to the project's `.mcp.json` so it's only active in this repo. Drop `-s project` for a user-scoped install (active everywhere). Use `-- vestige mcp --read-only` if you want browsing-only (no `record_*` tools).

Verify it's wired:

```bash
claude mcp list                 # vestige should appear
```

Then start a session in that repo and the six tools are available: `vestige_bootstrap`, `vestige_search`, `vestige_expand`, `vestige_get_project_context`, `vestige_record_observation`, `vestige_record_decision`.

Recommended agent flow:

1. At session start, call `vestige_get_project_context` to pull the project pack.
2. Use `vestige_search` to find relevant memories during work.
3. Use `vestige_expand` to read selected memories at higher fidelity.
4. Capture new decisions with `vestige_record_decision` so the next session inherits them.

A small CLAUDE.md hint that nudges the agent to do this:

```markdown
## Project memory

This repo uses Vestige (MCP server `vestige`). At the start of each session,
call `vestige_get_project_context` to load standing decisions and open
questions. Use `vestige_record_decision` when you make project-level calls.
```

## Where things live

```text
.vestige/config.toml            # in your repo, committed
~/.vestige/projects/<id>/memory.sqlite   # private store, on your machine
```

`.vestige/config.toml` pins the repo to a project scope. It carries no private data — commit it. The actual SQLite store lives outside the repo so it never accidentally lands in git.

`vestige status` always tells you exactly where the DB is.

## Design principles

These are tight constraints, not aspirations — they show up in `CODESTYLE.md` as enforceable rules:

- **Project-scoped by default.** A memory in repo A never affects repo B. Cross-project federation is a future opt-in (V0.7), not a default.
- **Progressive disclosure.** Memory returns compact handles first, expands on demand. Same shape for the code: types → public API → helpers.
- **Source-of-truth separation.** Durable journal (`memory_events`) ≠ derived interpretation (`memories`) ≠ disposable indexes (`memory_fts`).
- **Soft delete only in V0.** `forget` flips status; `restore` flips it back.
- **No daemon, no background threads.** Each CLI invocation opens SQLite, does its work, closes.
- **MCP exposes intent, not mechanics.** No raw SQL tools. No destructive defaults.

## Status

V0 is complete. All 12 PRD §23 Definition-of-Done items are shipped:

- `vestige init` / `status` (M0)
- Memory CRUD with soft delete + restore (M1)
- Deterministic progressive representations (M2 — folded into M1)
- FTS5 search and recall with composite ranking (M3)
- Project context pack (M4)
- MCP server with six tools, `--read-only` flag (M5)

42 tests passing across unit, store integration, and CLI/MCP smoke. `cargo clippy --all-targets -- -D warnings` clean.

## Roadmap

V0.1 adds embeddings (sqlite-vec) and hybrid FTS+vector search. V0.2 introduces the assimilation inbox for automatic capture. Full roadmap in `vestige_prd.md` §20.

## Contributing

- `vestige_prd.md` — the product spec. Every architectural decision traces back here.
- `CLAUDE.md` — short-form orientation for Claude Code (or any agent) editing this repo.
- `CODESTYLE.md` — the bar for PRs. Includes 7 non-negotiable Vestige-specific architecture rules.

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

## License

MIT.
