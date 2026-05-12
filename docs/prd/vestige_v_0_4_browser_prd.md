# Vestige V0.4 PRD — Memory Browser (TUI)

## 1. Product Summary

Vestige V0.4 introduces the **Memory Browser**: a full-screen interactive TUI (`vestige browse`) for inspecting and curating a project's memory store without leaving the terminal.

V0 proved explicit project memory. V0.1 added embeddings and hybrid recall. V0.2 added the assimilation inbox so capture became reviewable. V0.3 made the system inspectable end-to-end. V0.4 makes inspection **navigable**:

```text
 vestige browse
 ├── Memories     list → detail → why / sources / traces-of
 ├── Candidates   list → detail → approve / reject
 └── Traces       list → detail → replay / diff
```

The goal is not to add new data. V0.4 surfaces what already exists — memories, candidates, sources, events, traces — behind a navigable interface that respects progressive disclosure: card → summary → full body → provenance.

## 2. Product Thesis

The CLI is right for agents and scripts. It is wrong for a human curating a memory store at the end of a session.

The same human who happily runs `vestige search "auth"` in a shell loses patience when they want to:

```text
Scan the 12 candidates the inbox accumulated today.
Approve 4, reject 2 with a reason, leave 6 for later.
Forget that stale "use Diesel" decision from three weeks ago.
Replay the search trace where recall felt off this morning.
```

Each of those is two or three CLI commands plus copy-pasting IDs between them. A browser collapses each into a keystroke against a visible list.

V0.4 keeps Vestige aligned with its core principles:

- project-scoped memory
- human inspectability
- progressive disclosure
- no daemon, no background threads
- agent-safe defaults (the TUI is for humans; agent contract is unchanged)

## 3. Goals

V0.4 should enable Vestige to:

1. Launch a full-screen interactive browser via `vestige browse`.
2. Present three top-level tabs — Memories, Candidates, Traces — each a list+detail pair.
3. Navigate with vim keys (`j/k/g/G/q`) and arrow keys; switch tabs with `Tab`/`Shift-Tab`.
4. Live-filter the current list with `/` (lexical, FTS5).
5. Trigger hybrid search inside a tab with `:` (command palette).
6. Inspect a memory's provenance: `w` (why), `s` (sources), `t` (traces that returned it — **new in V0.4**).
7. Soft-delete and restore memories from the Memories tab (`f` / `r`, with confirm).
8. Approve and reject candidates from the Candidates tab (`a` / `R`).
9. Replay a trace from the Traces tab (`p`), showing the diff inline.
10. Bind directly to the Store with a single long-lived connection for the session — no daemon, no background threads.
11. Ship without a JSON/headless surface — the existing CLI covers scripting.
12. Be testable headlessly via `ratatui::backend::TestBackend` snapshot tests.

## 4. Non-Goals

V0.4 should not include:

- A background daemon — deferred to V0.5.
- Interactive memory capture (`record`/`remember`/`decision add` from inside the browser). Capture remains CLI/MCP; the browser is read+curate.
- Editing memory bodies. Memories are derived from the event journal; "edit" breaks that invariant.
- Source location fields (`line_range`, `commit_sha`, `byte_range`). Display will pick these up automatically when V0.5 ships them.
- A `--json` snapshot of the browser. The CLI already exposes the underlying data.
- A per-user colour theme. V0.4 ships one fixed palette plus `NO_COLOR` support.
- Mouse interaction. Keyboard only.
- A GUI dashboard.
- Cross-project browsing — strictly project-scoped, like all other V0.x surfaces.
- Embedded LLM summaries of memories or candidates.
- A persistent layout state (last tab, last selection). V0.4 starts fresh each launch.

## 5. Target User

Same primary user as V0.3 — solo developer or agent-heavy builder. The specific V0.4 user problems:

> "I have 12 candidates in the inbox. I want to triage them in one sitting, not run `vestige candidate show <id>` twelve times."

> "I want to see, at a glance, every decision memory in this project, sorted by importance, and forget the stale ones."

> "Recall felt off this morning. I want to scroll back through today's traces, see which queries returned what, and replay one."

V0.4 gives that user a keyboard-driven inspector over the data V0.3 already captured.

## 6. Core Concepts

### 6.1 Browser

A long-lived TUI process. Single Store connection held for the session lifetime. Reads on every redraw cycle are cheap because the data is local SQLite — no caching layer beyond the in-memory page.

### 6.2 Tab

One of `Memories`, `Candidates`, `Traces`. Each tab owns:

- A `ListState` (currently selected row, scroll offset)
- A `FilterState` (current `/` filter text, applied lexically)
- A `DetailState` (which sub-view of the selected row is showing: default / why / sources / traces-of / replay-diff)

Tabs are independent — switching does not reset state within a tab during a session.

### 6.3 Detail Pane

The right-hand panel. Default view is the selected item's compact representation (summary + key metadata). Sub-views replace the default for that selection only, indicated by a breadcrumb header (e.g. `mem_01HX7 › why`). `Esc` returns to default.

### 6.4 Command Palette

Opened with `:`. Single-line prompt at the bottom of the screen. Commands:

- `:search <text>` — re-list current tab using hybrid search (delegates to `vestige-engine::search_hybrid`)
- `:mode lexical|semantic|hybrid` — set list mode for the current tab
- `:kind decision|note|question|preference` — filter Memories by kind
- `:status active|deleted` — filter Memories by status
- `:caller cli|mcp` — filter Traces by caller
- `:goto <id>` — jump to a memory/candidate/trace by ID prefix
- `:help` — open help overlay (equivalent to `?`)
- `:quit` — exit (equivalent to `q`)

Tab autocompletes commands and known kinds.

### 6.5 Trace Forward-Link (new in V0.4)

V0.3 reserved this. V0.4 ships it: from a memory detail, `t` shows "which traces returned this memory" — a list of `trace_id`s where this memory appears in `result_ids_json`. Implementation is a `LIKE` scan over `query_events.result_ids_json`; with the default cap of 10000 traces this is acceptable. If a `query_events_results` join table becomes valuable later, V0.5+ can add it.

## 7. User Experience

### 7.1 Launch

```bash
vestige browse
```

Opens full-screen, renders the Memories tab, focuses the list pane. Status line at the bottom shows project ID and counts: `proj_abc · 47 memories · 3 candidates · 184 traces`.

### 7.2 Layout

```text
┌─ Vestige · proj_abc ──────────────────────────────────────────────────┐
│ [Memories(47)] [Candidates(3)] [Traces(184)]                          │
├───────────────────────────┬───────────────────────────────────────────┤
│> mem_01HX7  dec   0.82    │ mem_01HX7 · decision · 0.82               │
│  mem_01HX8  note  0.55    │ ─────────────────────────────────────────  │
│  mem_01HX9  q     0.40    │ Use FTS5 + vec hybrid for recall          │
│  mem_01HXA  pref  0.70    │                                            │
│  mem_01HXB  dec   0.65    │ summary: Hybrid search combines lexical    │
│  …                        │ recall (FTS5) with semantic recall …       │
│                           │                                            │
│                           │ events: 2  ·  sources: 1  ·  traces-of: 7 │
├───────────────────────────┴───────────────────────────────────────────┤
│ j/k move · / filter · : command · w why · s sources · t traces · ?   │
└───────────────────────────────────────────────────────────────────────┘
```

### 7.3 Memories Tab

**Keys:**

| Key | Action |
|---|---|
| `j` / `↓` | Next row |
| `k` / `↑` | Previous row |
| `g` | First row |
| `G` | Last row |
| `Ctrl-d` / `Ctrl-u` | Page down / up |
| `Enter` | Expand detail (full body) |
| `Esc` | Collapse / return to default detail |
| `w` | Show provenance walk (why) |
| `s` | Show sources |
| `t` | Show traces that returned this memory (forward-link) |
| `f` | Forget (with confirm `[y/N]`) |
| `r` | Restore (only when selected memory is `deleted`) |
| `/` | Open filter prompt |
| `:` | Open command palette |
| `Tab` / `Shift-Tab` | Next / previous tab |
| `?` | Help overlay |
| `q` | Quit |

**Detail default view** for a memory: kind, importance, status, summary representation, counts (events / sources / traces-of), timestamp.

**Detail `w` (why)**: templated provenance walk identical to `vestige why <id>` output (delegates to `vestige-store::provenance::fetch_memory_events` + sources). Rendered as a vertical timeline.

**Detail `s` (sources)**: list of `SourceKind` + content preview (capped at 240 chars), identical to `vestige sources <id>`.

**Detail `t` (traces-of)**: list of trace rows where this memory appears in `result_ids_json`. Empty state: "no traces returned this memory yet."

**Forget / restore:** flips status via existing `forget_memory` / `restore_memory`. Confirmation modal blocks input until `y` or `n`. Status badge updates in the list immediately.

### 7.4 Candidates Tab

**Keys:** same navigation; mutation keys differ.

| Key | Action |
|---|---|
| `a` | Approve (with confirm) |
| `R` | Reject (opens reason prompt; reason can be empty) |
| `w` | Show provenance walk |
| `s` | Show sources |

Approve and reject delegate to existing `mark_candidate_approved` / `mark_candidate_rejected`. On approve, the new `mem_<ULID>` is briefly shown in the status line.

### 7.5 Traces Tab

| Key | Action |
|---|---|
| `Enter` | Expand to full trace detail (parameters, results, scores) |
| `p` | Replay (delegates to engine; renders diff inline) |
| `:caller cli|mcp` | Filter by caller |
| `:kind search|expand|context` | Filter by kind |

Replay diff view: `added` / `removed` / `score_changes` as three lists. Provider mismatch surfaces as a banner.

### 7.6 Help Overlay

`?` opens a modal listing all bindings. `Esc` or `?` closes.

### 7.7 Empty States

- Memories empty: "No memories yet. Use `vestige remember`, `vestige decision add`, etc."
- Candidates empty: "Inbox empty. Candidates accumulate from auto-memorise."
- Traces empty: "No traces yet. Run a search or expand."

### 7.8 Resize

The browser handles `Resize` events. Two-pane ratio is fixed at 40/60 for terminals ≥120 cols; collapses to single-pane (Tab to toggle list/detail focus) below 100 cols.

## 8. Data Model

V0.4 adds no schema. It reads existing tables and views:

- `memories` (list, detail)
- `memory_events` (provenance walk)
- `memory_sources` (sources)
- `memory_provenance` view (why)
- `candidates` (list, detail)
- `candidate_events`, `candidate_sources` (candidate provenance)
- `query_events` (traces; also for trace forward-link via `LIKE` scan on `result_ids_json`)

No migration ships with V0.4.

## 9. CLI Requirements

### 9.1 `vestige browse`

```bash
vestige browse                # launch on the resolved project
vestige browse --tab traces   # open with a specific initial tab (refinement; default is memories)
```

Exits cleanly on `q`, `Ctrl-c`, or unhandled signal. Restores terminal state via `crossterm::terminal::disable_raw_mode` + leave-alt-screen, including on panic (wrap the run loop with a panic hook that restores).

### 9.2 No JSON output

`vestige browse` does not accept `--json`. The existing `list`, `show`, `search`, `why`, `sources`, `trace` commands cover all programmatic access.

## 10. MCP Requirements

**None.** The browser is a human-only surface. No new MCP tool, no extension of existing ones. The MCP contract is unchanged in V0.4.

This is intentional: agents already have full inspection via `vestige_expand` (incl. `depth=provenance`), `vestige_search`, `vestige_trace`, `vestige_list_candidates`, etc. A TUI is the wrong shape for agent consumption.

## 11. Search and Filtering Behaviour

### 11.1 `/` Filter

Lexical (FTS5) over the current tab's list. Re-runs `list_*` with a `query` parameter on every keystroke (debounced 80ms). Project-scoped like everything else.

### 11.2 `:search` Command

Hybrid search via `vestige-engine::search_hybrid`. Replaces the current list with ranked `MemoryCard`s. `Esc` clears and returns to the unfiltered list.

### 11.3 No Cross-Tab Search

Search is per-tab. There is no "search everything." This matches the CLI shape and avoids ambiguous result-type rendering.

## 12. Configuration

```toml
[browser]
default_tab = "memories"        # "memories" | "candidates" | "traces"
page_size = 200                  # rows fetched per page
filter_debounce_ms = 80
confirm_destructive = true       # confirm modal on forget/reject
```

All optional; defaults shown. Lives in `.vestige/config.toml` alongside the existing `[traces]` block. Round-trips through `vestige-config` like other blocks.

## 13. Implementation Plan

### Milestone 1 — Scaffolding

- Add `ratatui = "0.28"` and `crossterm = "0.28"` to workspace deps
- New file `crates/vestige-cli/src/commands/browse.rs`
- New module `crates/vestige-cli/src/browse/` with `app.rs` (state), `event.rs` (input), `ui.rs` (draw), `tabs/{memories,candidates,traces}.rs`
- Terminal setup/teardown with panic hook restoring terminal state
- Tab switching, quit, help overlay
- Empty stub tabs that render counts only

Acceptance: `vestige browse` launches, switches tabs, shows counts, quits cleanly. No data rendered yet.

### Milestone 2 — Memories Tab (read-only)

- List pane wired to `list_memories` with pagination
- Detail pane wired to `get_memory`
- Navigation keys (j/k/g/G/Ctrl-d/Ctrl-u/Enter/Esc)
- `/` filter (debounced) calling `search_memories` (lexical mode)

Acceptance: a project with N memories renders correctly; filter narrows the list; detail follows selection.

### Milestone 3 — Memory Provenance Sub-Views

- `w` calls `fetch_memory_events` + renders the templated walk
- `s` calls `fetch_memory_sources` + renders typed receipts
- `t` runs a `LIKE` scan over `query_events.result_ids_json` for traces-of (new helper on `trace_ops`: `fetch_traces_for_memory`)

Acceptance: each key surfaces the same content as the CLI equivalent; `t` returns 0 when no traces exist and grows as searches are run.

### Milestone 4 — Memory Mutations

- `f` forget with confirmation modal
- `r` restore (visible only when selected memory has `status=deleted`)
- Status badge updates in list after mutation

Acceptance: forget/restore round-trip; soft-deleted memories visible with strike-through style when `:status all` is set.

### Milestone 5 — Candidates Tab

- List + detail (same pattern as memories)
- `a` approve, `R` reject (with reason prompt)
- Provenance sub-views (`w`, `s`) reuse logic from memories

Acceptance: 5 candidates → triage all 5 from inside the browser; counts in tab header update.

### Milestone 6 — Traces Tab + Replay

- List wired to `fetch_traces`
- Detail renders parameters/results/scores via existing structures
- `:caller`, `:kind` filters
- `p` triggers engine replay; diff rendered as three lists
- Provider mismatch banner

Acceptance: traces list renders; replay produces the same diff as `vestige trace replay <id>`.

### Milestone 7 — Command Palette + Polish

- `:` palette with command parsing, autocomplete
- `:goto <id>` jumps across tabs as needed
- `:mode lexical|semantic|hybrid` for memories tab
- Status line counts + project ID
- `NO_COLOR` env var honoured
- Resize handling, narrow-terminal single-pane fallback

Acceptance: every documented binding works; resizing to 80 cols collapses cleanly.

### Milestone 8 — Docs and Demo

- README V0.4 section with screenshot or asciinema cast
- `docs/v0.4.md` walkthrough mirroring `docs/v0.3.md`
- Landing page (`docs/src/data.js`): V0.4 row → `done`; V0.5 (Daemon) → `now`
- CLAUDE.md "Milestones" updated

Acceptance: a new user can install, run `vestige browse`, and inspect their store without reading any other docs.

## 14. Testing Requirements

### Unit Tests

- Command palette parser: all documented commands round-trip; unknown commands return a typed error rendered as a status-line message
- Filter debounce logic
- Status formatting helpers

### Store Integration Tests

- `fetch_traces_for_memory` returns expected trace IDs after a search that includes the memory; returns empty for a memory never returned
- Project-scope isolation honoured (project A's memory never sees project B's traces)

### TUI Snapshot Tests (`ratatui::backend::TestBackend`)

- Memories tab renders correctly for: empty store, 1 memory, 200 memories (paging boundary)
- Detail pane renders correctly for each sub-view: default, why, sources, traces-of
- Help overlay renders
- Confirmation modal renders
- Resize from 120 to 80 cols collapses to single pane

### CLI Smoke Tests

- `vestige browse` launches in a pty harness, sends keystrokes, asserts terminal output (use `expectrl` or similar)
- `q` quits with exit code 0
- `Ctrl-c` quits with exit code 0 and restores terminal

### Regression Tests

- All existing CLI commands unchanged
- MCP surface unchanged (no new tool, no schema change to existing ones)
- V0.3 trace/provenance flows unchanged
- Soft-delete excludes from search (FTS trigger sync unchanged)

## 15. Acceptance Criteria

V0.4 is complete when:

- `vestige browse` launches a full-screen TUI bound to the resolved project's Store
- Three tabs render with live counts: Memories, Candidates, Traces
- Vim + arrow navigation works across all tabs
- `/` lexical filter narrows the current list in real time
- `:` command palette accepts every documented command
- Memory mutations (forget/restore) and candidate mutations (approve/reject) round-trip and update the visible list
- `w` / `s` / `t` reveal provenance, sources, and traces-of for a memory
- `p` replays a trace and renders the diff
- Terminal state is restored on quit, panic, or `Ctrl-c`
- `NO_COLOR` is honoured
- TUI snapshot tests cover the key states
- No new MCP tools and no schema changes
- README, PRD, and landing-page roadmap updated

## 16. Open Questions

1. Should `vestige browse` accept `--tab <name>` to open on a specific tab? **Decision (V0.4): yes — small, useful, low risk.** (Open for revisit if pain emerges.)
2. Should the Memories tab show soft-deleted entries by default? **Decision (V0.4): no — `:status deleted` or `:status all` opts in. Matches `vestige list` default.** (Open for revisit if pain emerges.)
3. Should there be a global undo for forget/approve/reject? **Decision (V0.4): no — `r` restores forgets; approvals are recorded but a memory can be forgotten right after; rejects are reasoned and final per V0.2.** (Open for revisit if pain emerges.)
4. Should the filter input support FTS5 operators (`AND`, `OR`, `"phrase"`)? **Decision (V0.4): yes — pass through verbatim to `search_memories` which already handles FTS5 syntax.** (Open for revisit if pain emerges.)
5. Should we pre-warm semantic vectors on browser start so `:mode semantic` is fast? **Decision (V0.4): no — first `:mode semantic` triggers the same cold path as the CLI. Pre-warm is daemon territory (V0.5).** (Open for revisit if pain emerges.)
6. Should the help overlay be data-driven from the bindings table or hand-rolled? **Decision (V0.4): data-driven — easier to keep in sync as bindings evolve.** (Open for revisit if pain emerges.)
7. Should two-pane ratio be configurable in `[browser]`? **Decision (V0.4): no for V0.4 — 40/60 is a sensible default; revisit if asked.** (Open for revisit if pain emerges.)
8. Should the panic hook also log a crash file under `~/.vestige/crashes/`? **Decision (V0.4): no — tracing to stderr is enough; add later if we get crash reports without repros.** (Open for revisit if pain emerges.)

## 17. Recommended First Slice

Build this first:

```bash
# 1. Scaffolding only — empty tabs, can switch and quit
vestige browse

# 2. Memories tab read-only
#    list renders, j/k/g/G work, detail follows selection

# 3. Live / filter on memories
```

No candidates. No traces. No mutations. No command palette. No sub-views.

Once that proves the ratatui plumbing end-to-end, layer on:

- Provenance sub-views (`w` / `s` / `t`)
- Memory mutations
- Candidates tab
- Traces tab + replay
- Command palette + polish
- Docs + demo

That keeps the first PR small, exercises the framework against real V0.3 data, and lets us catch terminal-handling edge cases before we widen the feature surface.

## 18. Implementation Drift (post-merge)

The implementation in PR #77 diverged from this PRD in four places. Recorded here so the PRD stays honest about what shipped vs. what was scoped.

- **§13 M1 — `ratatui` version**: PRD said `0.28`; shipped on `0.30` (the current release line at build time). `crossterm` shipped via ratatui's re-export rather than a separate dep.
- **§11.1 — `/` filter debounce**: PRD specified an 80 ms debounce. Implementation runs the FTS5 query synchronously on every keystroke instead — local SQLite returns in sub-millisecond time so the debounce added latency without solving a real problem. Revisit if a project gets large enough that single-keystroke search becomes perceptible.
- **§12 — `[browser]` config block**: not shipped. The defaults in V0.4 (40/60 split, no page-size limit beyond an internal 500-row cap, no debounce, confirm-on-destructive) are hardcoded. Add the block when the first user asks to tune one of them.
- **§7.8 — narrow-terminal single-pane collapse**: not shipped. Current 40/60 layout works down to ~80 cols; narrower terminals are out of scope until someone reports the pain.

Two PRD items that **did** ship but warrant a note on shape:

- **§6.4 — `:` palette command set**: ships `:quit`, `:help`, `:goto`, `:kind`, `:status`, `:caller`, `:search`. `:mode lexical|semantic|hybrid` is deferred — it needs the `[embeddings]` provider plumbed through the browser, which depends on infrastructure landing in V0.5+. Tab autocomplete is also deferred to V0.4.x polish.
- **§7.4 — reject prompt**: an empty buffer no longer submits `RejectionReason::Other("unspecified")`. The prompt re-opens with a status flash listing the typed presets (`duplicate / wrong / not_durable / too_noisy / stale`); `Esc` cancels. This is stricter than the PRD's "reason can be empty" wording and aligns with §16's "rejects are reasoned and final."
