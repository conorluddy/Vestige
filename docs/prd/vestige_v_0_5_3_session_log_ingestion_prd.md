# Vestige V0.5.3 PRD — Session-Log Ingestion

## 1. Product Summary

Vestige V0.5.3 introduces **opt-in session-log ingestion** — the first *passive* path by which memory candidates enter the system. It scans local coding-agent transcripts (Claude Code and Codex in v1), extracts candidate memories, redacts secrets, and files them into the V0.2 assimilation inbox as `Pending` candidates for human review.

Every prior milestone treated candidate creation as something an agent does *live, in conversation* (auto-memorise) or a human does *explicitly* (`vestige <kind> add`). V0.5.3 adds a third producer that mines transcripts after the fact — but it preserves the product's founding stance ("explicit capture over automatic ingestion", PRD §6, §13) by proposing into the **reviewed** inbox, never auto-promoting to memory. The inbox is the consent boundary.

Ingestion ships in **two modes that share one source layer**:

- **Agent-driven (zero-config default).** A new MCP tool, `vestige_scan_sessions`, hands the *currently-running agent* a batch of redacted, normalised transcript turns; the agent extracts what's worth keeping and calls the existing `vestige_propose_candidate`. No new model, no API key, no daemon — the extraction is done by whatever agent the developer already uses day-to-day.
- **Daemon (autonomous, opt-in).** The V0.5 daemon gains a scheduled `session_log_scan` job that extracts via a configurable `ExtractionProvider` (default `ollama`; `anthropic`/`openai` with a key) and proposes candidates unattended.

Both modes funnel through `vestige_engine::propose_candidate`. No new write paths. Off by default per project.

## 2. Product Thesis

The most valuable memories are the ones nobody remembered to record. A developer settles an architectural tradeoff at 18:00, closes the session, and the decision evaporates — auto-memorise only fires if the agent happened to notice it live, and explicit capture only happens if someone typed the command. Meanwhile the *entire* decision, with full reasoning, sits in a `.jsonl` transcript on disk.

V0.5.3's thesis: **the transcripts are already a high-signal corpus; we should mine them — on the user's terms.** The risks that kept this off the roadmap (auto-ingest noise, privacy, the "ingest everything" anti-pattern of V0.2 §non-goals) are all addressable by routing through the existing review inbox, redacting at the boundary, and shipping off-by-default. The candidate layer was built (V0.2) precisely so that low-confidence proposals could exist without polluting durable memory; session-log ingestion is the producer that layer was waiting for.

The daemon (V0.5) makes the *autonomous* variant cheap — a scan is just another scheduled sweep over a moving on-disk target, which is the daemon's whole reason to exist. But the daemon is headless and cannot borrow a subscription agent's auth, so the **default** mode is agent-driven: it asks the agent already in the room.

## 3. Goals

V0.5.3 should enable Vestige to:

1. Discover local session transcripts for **Claude Code** and **Codex**, map each to the correct Vestige project, and normalise turns behind a `SessionSource` trait.
2. Track per-source-file scan progress via a watermark so re-scans are incremental and idempotent.
3. Redact secrets from any transcript snippet before it is persisted.
4. Emit candidates via the existing `vestige_engine::propose_candidate` only, tagged with `SourceKind::SessionLog` provenance (session id + line range).
5. Ship **agent-driven mode** as the zero-config default: a `vestige_scan_sessions` MCP tool + a `vestige-scan-sessions` skill.
6. Ship **daemon mode** as opt-in autonomy: a `session_log_scan` job with a configurable `ExtractionProvider` (default `ollama`) and configurable model.
7. Be disabled by default; activate per project via an `[ingest]` config block.
8. Provide a one-shot `vestige scan` CLI for non-daemon, non-agent invocation.
9. Obey all existing invariants: soft-delete only, project-scope boundary, immutable migrations (one new numbered migration), newtype IDs, MCP-intent-not-mechanics.
10. Pass `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`.

## 4. Non-Goals

V0.5.3 should not include:

- **Auto-promotion to memory.** Candidates stay `Pending` in both modes. Promotion is the existing human/agent `approve` flow. No exceptions.
- **Source adapters beyond Claude Code and Codex.** Cursor (`~/.cursor`, SQLite), Gemini (`~/.gemini`), and Copilot are future `SessionSource` impls. Out of scope.
- **Real-time tailing of live sessions.** Daemon mode is a scheduled sweep; agent-driven mode is invoked, not continuous. (Note: V0.4.1 added a *display* Tail tab — unrelated; this is about ingestion, not the browser.)
- **A multi-LLM extraction *matrix* baked into the daemon's critical path.** The daemon's `ExtractionProvider` is configurable, but the headline "your day-to-day agent extracts" is delivered by **agent-driven mode**, not by the daemon impersonating that agent.
- **The daemon borrowing a subscription agent's auth.** Not technically possible (Claude Code's auth is not exposed to subprocesses); agent-driven mode is the answer.
- **V0.6 Directives allow/deny gating.** Candidates already pass through human review, so directive-based pre-filtering is a V0.6 refinement, not a V0.5.3 prerequisite. No ordering dependency.
- **Cross-project ingestion.** A session maps to exactly one project; subdir-cwd and no-matching-project sessions are skipped, not reassigned. Cross-project work waits for V0.7.

## 5. Target User

Same primary user as V0.5 — solo developer or agent-heavy builder. The V0.5.3 user problems:

> "We decided last week to drop the queue and go synchronous. I never recorded it. It's in some Claude Code transcript but I'll never find it."

> "I don't want to run Ollama just to get this. Can my agent just read its own logs?"

> "I'm fine with background scanning, but I do NOT want my API keys or `.env` contents ending up in a memory."

V0.5.3 solves all three: agent-driven mode needs no extra model; daemon mode is there for those who want unattended scanning; redaction + off-by-default + inbox review address the privacy concern.

## 6. Core Concepts

### 6.1 Source layer (shared)

A `SessionSource` trait abstracts each harness's on-disk format into a stream of `NormalizedTurn`s scoped to a Vestige project. Both extraction modes consume the same source layer; only the extraction step differs. Lives in `vestige-engine::ingest`.

### 6.2 Project mapping

Each discovered transcript must resolve to exactly one registered project's `repo_root`, or be skipped.

- **Claude Code**: `~/.claude/projects/<dash-encoded-cwd>/<uuid>.jsonl`. The directory name is the cwd with `/`→`-` (e.g. `-Users-conor-Development-Extoken` → `/Users/conor/Development/Extoken`). Map by decoding the dir name.
- **Codex**: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`. **Date-partitioned, not keyed by cwd** — the adapter reads the cwd from the session's in-file metadata. This divergence is the reason the trait exists.

A session whose cwd is a *subdirectory* of a repo maps to that repo's project; a session with no matching registered project is skipped cleanly (logged, never misattributed).

### 6.3 Scan cursor (watermark)

A new `session_scan_cursors` table records, per `(source, file_path)`, the last-scanned position (byte offset / line). Re-scans resume from the watermark. **Both modes advance the same cursor**, so a daemon sweep and an agent-driven scan never double-surface the same turns. The `propose_candidate` FTS dedup probe is the second safety net.

### 6.4 Redaction

A pure redaction pass scrubs secret-shaped content (API keys, tokens, `.env`-style `KEY=value`, private-key headers) from any snippet before it is stored as `source_content`. Applied in both modes, before the 2 KiB cap.

### 6.5 Extraction modes

- **Agent-driven**: `vestige_scan_sessions` returns redacted, cursor-advanced turns + provenance refs. The agent extracts and calls `vestige_propose_candidate`. The tool advances the cursor on read.
- **Daemon**: the `session_log_scan` job pulls turns, calls `ExtractionProvider::extract`, redacts, and calls `propose_candidate` directly. Provider unavailable ⇒ no-op + logged warning; it never dumps raw turns as candidates.

### 6.6 Provenance

Every ingested candidate carries a `SourceKind::SessionLog` receipt with `source_ref` = session id + line range and a redacted `source_content` snippet. Surfaces in `vestige why` / `vestige sources` and `vestige_expand depth=provenance`.

## 7. User Experience

### 7.1 Enable per project

```toml
# .vestige/config.toml
[ingest]
enabled = true                 # default false — nothing scans until set
sources = ["claude_code", "codex"]   # default: all supported
max_candidates_per_scan = 20   # firehose cap, per project per scan
min_importance = 0.5           # floor for proposed candidates
```

### 7.2 Agent-driven (default)

At session start (or on demand), the `vestige-scan-sessions` skill triggers the agent to call `vestige_scan_sessions`, review the returned turns, and propose candidates for anything worth keeping. Zero extra setup. Output: `Proposed N candidates from M recent turns. Review with vestige inbox.`

### 7.3 Daemon (autonomous, opt-in)

```toml
[extraction]
provider = "ollama"            # or "anthropic" | "openai" | "fake"
model = "llama3.1:8b"          # configurable; documented defaults per provider

[daemon]
session_log_scan_interval_secs = 1800
```

```bash
vestige daemon kick scan       # one-off sweep across supervised projects
```

### 7.4 One-shot CLI

```bash
vestige scan                   # scan this project's sessions once (uses [extraction] provider)
vestige scan --dry-run         # show what would be proposed, write nothing
```

## 8. Data Model

One new migration, `0006_session_scan_cursors.sql` (0005 is the latest shipped):

```sql
CREATE TABLE session_scan_cursors (
    source        TEXT NOT NULL,           -- "claude_code" | "codex"
    file_path     TEXT NOT NULL,
    project_id    TEXT NOT NULL,
    last_offset   INTEGER NOT NULL,        -- byte offset (or line) scanned through
    last_scanned_at TEXT NOT NULL,
    PRIMARY KEY (source, file_path)
);
```

No change to `memories`, `candidates`, `memory_sources`, or FTS. `SourceKind::SessionLog` is a new enum variant in `vestige-core`; `source_type` is free-form TEXT, so the receipt needs no schema change.

## 9. CLI Requirements

### 9.1 `vestige scan`
One-shot scan of the current project's sessions using the configured `[extraction]` provider. `--dry-run` writes nothing. Honours `[ingest]`. Text + `--json`.

### 9.2 `vestige daemon kick scan`
Adds `scan` to the existing `daemon kick` job set; triggers `session_log_scan::run_once` across supervised projects over the IPC socket.

## 10. MCP Requirements

### 10.1 `vestige_scan_sessions` (new)
Returns a batch of redacted, normalised, cursor-advanced turns for the current project plus provenance refs. Intent: *"give me recent un-reviewed transcript turns so I can extract memories."* Advances the cursor on read. Maps 1:1 to the source layer; exposes no SQL, no raw file paths beyond provenance refs. Pairs with the existing `vestige_propose_candidate` for the write step. Structured `{code, message, retryable}` errors.

## 11. Architecture

### 11.1 Crate structure

- **Source layer** (`SessionSource`, adapters, normalisation, cursor, scan orchestration) → `vestige-engine::ingest`. Both `vestige-mcp` and `vestige-daemon` already depend on engine, so neither reaches around it.
- **`ExtractionProvider`** (daemon mode only) → **new `vestige-extract` crate**, mirroring `vestige-embed`: sync trait, `fake` always-compiled, feature-gated `ollama`/`anthropic`/`openai`, `build_provider` factory, `ExtractError` with `ProviderDisabled(&'static str)` vs `UnknownProvider`, configurable model. Dep edge: `vestige-engine → vestige-extract → vestige-core`. Agent-driven mode never touches this crate.
- **Redaction** + **`SourceKind::SessionLog`** → `vestige-core`.

### 11.2 Daemon job (additive, mirrors embed_sweep/trace_prune)

`jobs/session_log_scan.rs::run_once(registry)`, `WorkerCommand::ScanSessionLogs`, `ResolvedDaemonConfig.session_log_scan_interval_secs`, a `select!` arm + `TickState` field, `JobKind::SessionLogScan`, `KickJob::ScanSessionLogs` (daemon + CLI). A per-project `build_project_extraction_provider` mirrors the existing `build_project_provider`. No existing job code modified.

### 11.3 Invariants that carry forward

Soft-delete only; project-scope boundary (a session for project A never yields a candidate in B); immutable migrations (one new file); newtype IDs; no new write paths (everything funnels through `propose_candidate`).

## 12. Config Schema

```toml
[ingest]
enabled = false                       # per-project master switch
sources = ["claude_code", "codex"]
max_candidates_per_scan = 20
min_importance = 0.5

[extraction]                          # daemon mode only
provider = "ollama"                   # "ollama" | "anthropic" | "openai" | "fake"
model = "llama3.1:8b"
# api_key via env for anthropic/openai; never stored in committed config

[daemon]
session_log_scan_interval_secs = 1800 # 0 disables the scheduled job
```

All `Option` fields with documented defaults, preserving round-trip fidelity (per the V0.5 config convention). `[extraction]` mirrors `[embeddings]` exactly (section struct + `From<&Section>` + `extraction_config_for`).

## 13. Scheduling Cadences

| Job | Default interval | Disable |
|-----|------------------|---------|
| `session_log_scan` | 1800 s | `session_log_scan_interval_secs = 0` |

Live-reloadable via `daemon.reload_config` like the existing job intervals.

## 14. Implementation Plan

Epic + 9 atomic child issues across 4 waves (no shared-file edits within a wave). **Waves 1–2 alone ship a complete, zero-config feature (agent-driven mode);** the daemon (A, G) is optional autonomy and may be split to V0.5.4 if faster delivery of the default path is preferred.

- **Wave 1** (source foundations): B `SessionSource` + Claude Code adapter · C cursor migration + store API · D redaction pass · E `SourceKind::SessionLog` · A `vestige-extract` crate.
- **Wave 2** (default mode + 2nd source): **I** `vestige_scan_sessions` MCP tool + skill · F Codex adapter.
- **Wave 3** (autonomous): G daemon `session_log_scan` job.
- **Wave 4**: H `vestige scan` CLI + docs (`docs/v0.5.3.md`, both modes + provider setup) + `docs/src/data.js` V0.5.3 entry + CLAUDE.md.

Full decomposition with labels and file lists: `~/.claude/plans/vestige-v053-session-log-ingestion.md`.

> Release-pipeline note: add `vestige-extract` to the topological publish order
> (`core → embed → extract → store → config → engine → mcp → cli`); never place a
> sibling crate in `[dev-dependencies]`.

## 15. Testing Requirements

### Integration tests (primary line of defence)
- Claude Code + Codex adapters produce candidates from fixture transcripts in a `TempDir`.
- Project-scope: a fixture session for project A yields no candidate in project B.
- Subdir-cwd maps to the right project; no-matching-project session is skipped.
- Re-scan is idempotent: second run over an unchanged file proposes nothing new (cursor + dedup).
- Agent-driven path: `vestige_scan_sessions` returns redacted, cursor-advanced turns; cursor advances on read.
- Daemon provider absent ⇒ `session_log_scan` no-ops with a warning, writes nothing.

### Unit tests
- Redaction scrubs key/token/`.env`/private-key patterns; leaves benign text intact.
- Claude Code dir-name ↔ cwd decode round-trip.
- Codex cwd-from-metadata extraction.
- Cursor watermark arithmetic at UTF-8 / line boundaries.

### Invariants to exercise
- Candidate carries `SourceKind::SessionLog` provenance with session id + line range.
- Snippet ≤ 2 KiB, truncated at a UTF-8 boundary, redacted before store.
- Migration validates and applies cleanly to an empty DB.

## 16. Acceptance Criteria

1. Disabled by default; only scans when `[ingest].enabled = true`.
2. Agent-driven mode works zero-config (no Ollama, no key).
3. Daemon mode works with a configured provider; model is configurable.
4. Both adapters produce candidates from real local logs, correctly project-scoped.
5. Snippets are redacted; secrets do not reach the inbox.
6. Re-runs are idempotent across both modes (shared cursor + dedup).
7. Provider-absent daemon scan no-ops safely.
8. `vestige scan`, `vestige scan --dry-run`, and `vestige daemon kick scan` all work.
9. No existing CLI or MCP surface breaks.
10. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace` green.

## 17. Open Questions / Future Work

- **Daemon mode → V0.5.4?** Splitting A+G out lands the zero-config default sooner and de-risks the provider matrix. Decision pending.
- **More sources** — Cursor (SQLite), Gemini, Copilot adapters as future `SessionSource` impls.
- **V0.6 Directives** become an optional pre-filter over what ingestion proposes (allow/deny by path glob / kind), reducing inbox noise.
- **Shared extraction engine with #93 (historian spike)** — both mine a corpus into candidates; `vestige-extract` is the natural common engine if #93 proceeds. Keep them converged.
- **Redaction completeness** — pattern-based redaction is best-effort; the inbox review + off-by-default posture remain the primary guarantees.

## 18. References

- Roadmap ordering: `docs/src/data.js` (canonical).
- V0.2 Assimilation Inbox PRD — the candidate layer this feature produces into.
- V0.3 Provenance PRD — `SourceKind`, source receipts.
- V0.5 Daemon PRD — the runtime this feature's autonomous mode extends.
- Plan: `~/.claude/plans/vestige-v053-session-log-ingestion.md`.
- Related: issue #93 (historian spike), issue #88 (V0.5.2 menu-bar controls).
