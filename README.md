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
vestige search "architecture"               # one-liner cards, ranked (fixed --limit 8)
vestige recall "architecture"               # same engine; --limit from [recall] max_results in config
vestige list --type decision --json         # machine-readable
vestige show mem_01HXXXXXXXXXXXXXXXXXX --depth full
vestige context --budget-tokens 1200        # the full project pack
```

Soft-delete and restore are first-class:

```bash
vestige forget   mem_01HXXXXXXXXXXXXXXXXXX
vestige restore  mem_01HXXXXXXXXXXXXXXXXXX
```

Candidate inbox (V0.2):

```bash
vestige candidate add --type decision \
  --body "Prefer append-only migrations — existing DBs cannot re-run mutated files." \
  --importance 0.9

vestige inbox                                # list pending candidates
vestige inbox show cand_01HXXXXXXXXXXXXXXXXXX
vestige approve cand_01HXXXXXXXXXXXXXXXXXX   # promotes to mem_<ULID>
vestige reject  cand_01HXXXXXXXXXXXXXXXXXX --reason not_durable
```

Every command supports `--json` for scripting. `VESTIGE_LOG=debug` turns on structured stderr logs.

## Semantic recall (V0.1)

V0 ships with BM25 lexical search. V0.1 adds embeddings and hybrid recall so agents can find memories that don't share keywords with the query. Embeddings are an optional, rebuildable index over the canonical SQLite store — the lexical path always works, even with no embeddings.

### Walkthrough

Continuing from the same project you initialised above:

```bash
vestige embed --all
# → Embedded 4 representations across 2 memories using provider=fake model=deterministic-sha256
# → Embedded 4; skipped 0; failed 0.

vestige embeddings status
# → Provider:  fake
# → Model:     deterministic-sha256
# → Memories:                    2 active
# → Embeddable representations:  4
# → Embedded representations:    4
# → Stale embeddings:            0

vestige search "canonical store" --mode hybrid
# → mem_01K…WWG decision  0.360  Use SQLite as the canonical local store
# →     [fts=0.500 vec=0.035 imp=0.700 type=0.800]

vestige search "fast scans" --mode semantic --json
# → {"mode":"semantic","results":[{"id":"mem_01K…XHJ","title":"Brute-force…",
# →   "score":0.387,"score_parts":{"fts":0.0,"vector":0.387,
# →   "importance":0.0,"type_boost":0.0,"total":0.387}, …}], "warnings":[]}
```

The convenience aliases `--lexical` / `--semantic` / `--hybrid` are equivalent to `--mode <name>`. Pass `--score-parts` on lexical or semantic mode to force the per-component breakdown into the JSON output (always on for hybrid).

### Choosing a mode

| Mode | Best for | Notes |
|------|---------|-------|
| `lexical` (default) | Exact keywords, IDs, command names, error strings. | Always available. BM25 over FTS5. |
| `semantic` | Paraphrases and concept queries — *"why did we pick our store?"*. | Requires `vestige embed --all` first. Hard error in MCP if no embeddings exist. |
| `hybrid` | The default for agents. Merges both legs with score diagnostics. | Falls back to lexical (with a warning) when embeddings are missing. |

`vestige recall` shares the same engine; the only difference is `--limit` defaults to `[recall] max_results` from config rather than a fixed `8`.

### Real semantic quality (recommended for production use)

The default `fake` provider is deterministic and exists for tests — it does not produce semantically meaningful vectors. For real recall, build with the `fastembed` feature, which downloads BAAI/bge-small-en-v1.5 (~60 MB, cached at `~/.vestige/models/`) on first use:

```bash
cargo install vestige --features fastembed
```

```toml
# .vestige/config.toml
[embeddings]
provider = "fastembed"
```

Or use Ollama (build with `--features ollama`):

```toml
[embeddings]
provider = "ollama"
model = "nomic-embed-text"
```

### Known limitations

- **Embeddings are an index, not state.** Memories are canonical in SQLite. `vestige reindex --embeddings` rebuilds the vector layer at any time; deleting it never loses memory. Hybrid mode falls back to lexical (with a warning) when embeddings are missing or were produced under a different provider/model/dimensions.
- **Switching provider/model/dimensions is detected, not auto-cleaned.** When the configured provider drifts away from what the embeddings were generated under, `vestige search` prints a warning at query time and falls back. The stored rows stay until you run `vestige reindex --embeddings` (or `vestige embed --all` after a clean re-index) — automatic stale-sweep is deferred to V0.5+.
- **Brute-force cosine scan, no `vec0` yet.** V0.1 reads all in-project, matching-provider vectors and ranks them in Rust. Comfortable to roughly 10k vectors per project; past that, semantic-mode latency starts to show. A future release will swap in a `vec0` virtual table behind the same `Store` API — the canonical store schema and the engine surface stay unchanged.

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

## Skills (agent skills)

Vestige ships with ten agent skills bundled into the binary, compliant with the [agentskills.io](https://agentskills.io) open standard. They turn the CLI into an ambient capability — the agent fires the right `vestige` command at the right moment without you having to prompt for it.

Currently consumed by **Claude Code** (reads `.claude/skills/`) and **Codex** (reads `.agents/skills/`). `vestige init` writes to BOTH dirs by default so any compliant agent can pick them up.

```bash
# vestige init installs to BOTH .claude/skills/ and .agents/skills/ by default
vestige init --name "My Project"

# or, in an existing repo (still writes to both):
vestige skills install

# target a single dir if you only use one agent:
vestige skills install --target claude     # .claude/skills/ only
vestige skills install --target agents     # .agents/skills/ only

# inspect what shipped with this binary:
vestige skills list --json

# opt out at init time:
vestige init --no-install-skills
```

Re-running `skills install` is idempotent — files that match the bundled bytes are skipped. If you've hand-edited a SKILL.md, install hard-fails with a verbose drift report; pass `--force` to overwrite.

The ten skills, by role:

| Role        | Skill                       | Wraps                          |
|-------------|-----------------------------|--------------------------------|
| Auto        | `vestige-auto-memorise`     | dispatches inline to `vestige <cmd> add` |
| Capture     | `vestige-record-decision`   | `vestige decision add`         |
| Capture     | `vestige-record-note`       | `vestige note add`             |
| Capture     | `vestige-record-preference` | `vestige preference add`       |
| Capture     | `vestige-record-question`   | `vestige question add`         |
| Retrieve    | `vestige-context`           | `vestige context`              |
| Retrieve    | `vestige-recall`            | `vestige recall`               |
| Retrieve    | `vestige-show`              | `vestige show`                 |
| Lifecycle   | `vestige-forget`            | `vestige forget`               |
| Lifecycle   | `vestige-restore`           | `vestige restore`              |

`vestige-auto-memorise` is the headline one: it watches for memorable moments (decisions, preferences, open questions, TILs, gotchas) and captures them without an explicit "remember this" prompt. The other capture skills handle explicit cues; the retrieve and lifecycle skills give the agent durable read + edit affordances.

Skills shell out to the `vestige` binary — they don't depend on the MCP server being configured, but they compose well alongside it.

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
- **No daemon in V0–V0.4; opt-in daemon from V0.5.** Each CLI invocation opens SQLite, does its work, closes. V0.5 adds an opt-in per-host LaunchAgent for scheduled maintenance jobs — it coexists with one-shot CLI/MCP via WAL.
- **MCP exposes intent, not mechanics.** No raw SQL tools. No destructive defaults.

## What's shipped

### V0.4 — Memory browser (TUI)

V0.4 adds an interactive terminal browser over the project's memory store. Three tabs, two-pane layout, full keyboard-driven navigation, and every V0–V0.3 read+mutate surface reachable from a single binary.

- `vestige browse` — launches a full-screen browser. Bound to the project resolved from `.vestige/config.toml`. Single Store handle for the session; no daemon.
- **Memories tab** — list/detail over `list_memories`; `/` opens a per-keystroke FTS5 filter; soft-deleted entries strike-through inline; `w` / `s` / `t` reveal provenance walk, typed source receipts, and the new trace forward-link.
- **Candidates tab** — list/detail over `list_candidates`; `a` approves (with confirm); `R` (Shift+r) rejects with a reason prompt that parses `duplicate / wrong / not_durable / too_noisy / stale / <freeform>`.
- **Traces tab** — list/detail over `query_events`; `p` replays the selected trace via `vestige_engine::replay_trace` and renders the added / removed / score-change diff inline. Provider-mismatch and mode-fallback surface as inline banners.
- `f` / `r` — forget / restore memories with a y/N confirm modal. Status flash announces the outcome in the status row.
- `:` command palette — `:goto <id>` jumps across tabs by ID prefix (`mem_` / `cand_` / `trace_`); `:kind <type>` and `:status active|deleted|all` filter Memories; `:caller cli|mcp` filters Traces; `:search <text>` mirrors `/`; `:help` and `:quit` are aliases for `?` and `q`.
- `NO_COLOR` env var honoured. `?` opens the full keymap overlay. `Esc` precedence: modal > palette > help > filter focus > sub-view.
- New store helpers: `Store::pending_candidate_count` and `Store::fetch_traces_for_memory` (the V0.3-reserved trace forward-link, anchored on `"<full id>"` in the JSON to avoid ULID substring collisions).

The browser is interactive-only — pipe-friendly inspection still lives in `list`, `show`, `search`, `why`, `sources`, `trace`. Running `vestige browse` without a TTY fails fast with a friendly message.

See [`docs/v0.4.md`](docs/v0.4.md) for the full walkthrough. Full spec: [`docs/prd/vestige_v_0_4_browser_prd.md`](docs/prd/vestige_v_0_4_browser_prd.md).

### V0.5 — Daemon Runtime (in progress)

Opt-in per-host daemon for scheduled maintenance jobs. Coexists with one-shot CLI/MCP via WAL.

- Periodic embed sweep across all known projects
- Daily trace VACUUM
- Optional candidate stale-TTL
- LaunchAgent install on macOS (`vestige daemon install`)
- CLI controller: `vestige daemon {start,stop,status,kick,install,uninstall,log}`
- Status: JSON status file + Unix-domain control socket
- Optional `Vestige.app` SwiftUI menu-bar UI (parallel track)

Spec: `docs/prd/vestige_v_0_5_daemon_prd.md`. Walkthrough: `docs/v0.5.md`.

### V0.3 — Provenance and Receipts

V0.3 makes the memory store **inspectable end-to-end**. Every memory is now answerable to "where did this come from?" and every recall is answerable to "what did the agent ask, and what did it get?".

- `vestige why <mem_or_cand_id>` — templated provenance walk: recorded event, candidate back-reference (if promoted from the inbox), source receipts, and full status history.
- `vestige sources <id>` — raw typed source rows for any memory or candidate, filterable by kind (`file`, `commit`, `url`, `agent_session`, `mcp_call`, `candidate`, `manual`).
- `vestige trace` / `vestige trace <trace_id>` — list and inspect the `query_events` log. Every `search`, `expand`, and `context` call now writes one trace row, tagged `caller=cli` or `caller=mcp`.
- `vestige trace replay <trace_id>` — re-run a stored trace against the current store; diffs added / removed / score-changed memories explicitly.
- `vestige_expand depth=provenance` (MCP) — structured provenance walk over the MCP surface without adding a new tool.
- `vestige_trace` (MCP) — new tool; `action=list|show|replay` for agent-side trace inspection.
- `[traces]` config block — tune the FIFO cap (`max_per_project`, default 10 000), `query_text` truncation, and per-surface (`cli` / `mcp`) toggles. Safe to omit — all defaults are production-ready.

All provenance and trace data is project-scoped and never leaks across repos.

See [`docs/v0.3.md`](docs/v0.3.md) for the full walkthrough. Full spec: [`docs/prd/vestige_v_0_3_provenance_prd.md`](docs/prd/vestige_v_0_3_provenance_prd.md).

### V0.2 — Assimilation inbox

V0.2 adds a review layer between agent capture and durable memory. Agents propose candidates (`cand_<ULID>`) that queue in an inbox, invisible to recall, until a human approves or rejects them. This keeps the memory store trustworthy — everything recalled has been seen by a human, not just emitted by an LLM.

- `vestige candidate add` — propose a candidate with type, body, rationale, confidence, importance, and optional source attachment.
- `vestige inbox` / `vestige inbox show` — list and inspect pending candidates.
- `vestige approve` — promote a candidate to a full `mem_<ULID>` with full provenance.
- `vestige reject` — dismiss with a reason (`duplicate`, `wrong`, `not_durable`, `too_noisy`, `stale`). Rejected candidates are audited but never recalled.
- Three new MCP tools: `vestige_propose_candidate`, `vestige_list_candidates`, `vestige_get_candidate`. Approval/rejection tools are CLI-only until the review policy is proven.
- `vestige-auto-memorise` skill now proposes candidates rather than writing durable memories. Explicit capture skills (`vestige-record-decision` etc.) still write directly.

See [`docs/v0.2.md`](docs/v0.2.md) for the full walkthrough.

### V0.1 — Semantic recall

V0.1 adds embeddings and hybrid recall so agents can find memories that don't share keywords with the query. See the [Semantic recall (V0.1)](#semantic-recall-v01) section below.

### V0 — Core memory layer

All 12 PRD §23 Definition-of-Done items are shipped:

- `vestige init` / `status` (M0)
- Memory CRUD with soft delete + restore (M1)
- Deterministic progressive representations (M2 — folded into M1)
- FTS5 search and recall with composite ranking (M3)
- Project context pack (M4)
- MCP server with six tools, `--read-only` flag (M5)

## Roadmap

V0.5 (Daemon runtime) is the active milestone — in progress on `feat/v0.5-daemon`. Full roadmap in `vestige_prd.md` §20 — note the landing-page order: V0.4 = browser (shipped), V0.5 = daemon (in progress), V0.6 = directives.

## Contributing

- `vestige_prd.md` — the product spec. Every architectural decision traces back here.
- [`docs/prd/vestige_v_0_3_provenance_prd.md`](docs/prd/vestige_v_0_3_provenance_prd.md) — V0.3 Provenance and Receipts spec.
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
