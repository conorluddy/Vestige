---
name: vestige-review
description: 'Review Vestige code against this repo''s CODESTYLE.md, CLAUDE.md, and PRD — naming, progressive disclosure, error handling, agentic patterns, and the seven hard architectural rules (soft-delete only, project-scope boundary, immutable migrations, one-way crate deps, newtype IDs, MCP intent-not-mechanics, no daemon). Use when the user says "vestige-review", "review this", "review my branch", "check my code", "look over this PR/diff", "roast this", "is this ready to merge", or after finishing work in any Vestige crate. Prefer over the global code-review skill inside this repo — it layers Vestige-specific rules on top of the style guide and produces findings grouped by severity (blocker / major / minor / nit) with file:line citations and concrete fixes.'
---

# Vestige code review

Produce a structured, severity-grouped review against this repo's rules. Cite the rule each finding violates so the author can verify quickly. Findings without citations and concrete fixes are noise — don't write them.

## 1. Scope the review

Default scope, in order of preference:

1. If the user named a PR (`#123`, a URL): use `gh pr diff <n>` and `gh pr view <n>`.
2. If the user named files / a directory: review those.
3. If a feature branch is checked out: `git diff main...HEAD` plus `git status` for uncommitted changes.
4. Otherwise: review staged + unstaged changes (`git diff HEAD`).

Confirm the scope in one line at the top of the review ("Reviewing branch `feat/xyz` vs `main`, 4 files, 213 additions").

Read the actual file contents at HEAD (not just the diff) when a finding depends on file-level structure (progressive disclosure, file length, section banners).

## 2. Load the rulebooks

Before grading, read these once per review so citations are accurate:

- `CODESTYLE.md` — full style guide and the pre-PR checklist.
- `CLAUDE.md` — the seven hard rules and conventions.
- `vestige_prd.md` — only when a finding cites a PRD section (§5.1, §11.5, etc.).

Don't paraphrase from memory if you're not sure — open the file.

## 3. Hard rules (blockers)

These are non-negotiable. Any violation is a `blocker` and the review verdict is "blockers present" regardless of how clean the rest is.

| # | Rule | What to grep for |
|---|---|---|
| 1 | **Soft-delete only** (CLAUDE.md "Hard rules") | `DELETE FROM memories`, `DROP`, hard-delete helpers in non-test code |
| 2 | **Project-scope boundary** (PRD §5.1) | memory reads/writes without a `project_id` filter; `ProjectId` resolved from anywhere other than the current `.vestige/config.toml`; cross-project iteration |
| 3 | **Immutable migrations** | edits to existing files under `crates/vestige-store/src/migrations/`; non-numbered migration filenames; out-of-order numbers |
| 4 | **One-way crate deps** (CLAUDE.md "Architecture") | `vestige-core` importing `rusqlite`, `clap`, `rmcp`, or any other Vestige crate; `vestige-config` importing `store`/`cli`/`mcp`; `vestige-mcp` importing `clap` |
| 5 | **Newtype IDs everywhere** | bare `String` / `&str` parameters that should be `MemoryId`, `ProjectId`, `EmbeddingId`, or event IDs |
| 6 | **MCP intent-not-mechanics** (PRD §13.1, §13.9) | raw-SQL MCP tools; destructive defaults; tools returning unstructured errors instead of `{code, message, retryable}`; one tool per memory type instead of a semantic dispatcher |
| 7 | **No daemon, no background threads in V0** (PRD §8) | `std::thread::spawn`, `tokio::spawn` outside the MCP transport, long-lived background tasks, sleep loops |

Also blocker-level:

- **Bytes-not-chars** truncation of source snippets — must be byte-bounded with a UTF-8 codepoint-boundary cut at 2 KiB (PRD §8).
- **Hardcoded `.vestige`** strings outside `vestige-config` — use `CONFIG_DIR` / `CONFIG_FILE`.
- **`unwrap()` / `panic!` / `expect()` on user-facing CLI or MCP paths.**
- **Silent `let _ = result;`** without a comment explaining why dropping the error is safe.

## 4. Major findings

Things that are wrong but won't break production:

- **Progressive disclosure violated** — file doesn't open with module doc → types → public API → private helpers → tests. Helpers above public API. Public API buried at the bottom.
- **Error layering wrong** — `anyhow` used inside a non-CLI crate; `unwrap_or_default` swallowing a domain error; MCP path returning an `anyhow::Error` instead of structured `{code, message, retryable}`.
- **Missing typed contract** — `serde_json::Value` in/out of a public function; stringly-typed status / type / depth where an enum exists.
- **Non-idempotent mutation** without `create_*` naming — `init`, `ensure_*`, `remember` style operations must be safely re-runnable.
- **Missing `--json` on a CLI command that prints results** — agents need it.
- **Tests use mocks instead of `TempDir`** for SQLite-touching code.
- **Missing test for an invariant** that flows from CLAUDE.md "Invariants that deserve dedicated tests" (1–7), e.g. soft-delete excludes from search, restore re-indexes, cross-project isolation.
- **Section banners missing** in files >150 lines with multiple logical groups.
- **`memory_events` bypassed** for a mutation — every state change must journal (PRD §11.5).

## 5. Minor / nit

- Verbose-naming violations: abbreviations (`repr`, `cfg`, `ctx`), missing units (`delay` not `delay_ms`), vague names (`data`, `value`, `result` for domain types), function names that aren't verbs.
- Comments describing **what** instead of **why** — if the code is self-explanatory, delete the comment; if not, rename until it is.
- Nested conditionals where guard clauses would flatten the happy path.
- Unstructured `tracing` calls — should include `project_id`, `memory_id`, `operation`, `duration_ms` as fields, not interpolated into the message.
- File >300 lines without a clear split point flagged.
- Premature abstraction (extraction at the second duplication; a trait with one impl) — AHA over DRY.

Promote a minor to major if it appears repeatedly across the diff — pattern, not incident.

## 6. What not to flag

- Style preferences not in `CODESTYLE.md`.
- Missing tests for trivial getters, `Display` impls, or serde round-trips already covered by one happy-path test.
- "Could be more generic" — V0 first; the PRD's deferred items are deferred for a reason.
- Clap argument parsing details — clap's job.
- Speculative concurrency / performance concerns when the code is straight-line synchronous SQLite (it's fast).
- Doc nits on private helpers.

## 7. Output format

Open with one line of scope + a verdict, then findings grouped by severity. Skip empty severity sections. Close with the gate command.

```
Reviewing <scope> — <N> files, <+adds/-dels>.
Verdict: blockers present | needs changes | ready to merge

## Blockers
- `crates/vestige-core/src/memory.rs:42` — bare `String` for memory id, must be `MemoryId` (CLAUDE.md "Hard rules", CODESTYLE.md "Newtypes for Validation at Boundaries").
  Fix: change signature to `fn forget_memory(store: &mut Store, id: &MemoryId) -> Result<()>`.

## Major
- `crates/vestige-cli/src/commands/search.rs:1-220` — public `run()` is at the bottom; helpers above (CODESTYLE.md "File-Level Disclosure").
  Fix: move `run` above the helpers; add a `// === PUBLIC API ===` banner.

## Minor
- `crates/vestige-store/src/lib.rs:88` — `cfg` should be `config` (CODESTYLE.md "Naming" rule 3).

## Nit
- `crates/vestige-core/src/representations.rs:14` — comment restates what the code says; delete.

Gate: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
```

Each finding: `file:line` — one-sentence problem with rule citation — concrete fix on the next line. No platitudes ("consider improving"), no vague verbs ("refactor this"). If you can't write a concrete fix, you don't understand the issue well enough to flag it yet.

## 8. Tone

Direct, technical, brief. The author wants to know what's wrong and how to fix it, not be congratulated. Don't pad with "Overall the code is well-written" — the verdict line carries that. Don't soften blockers; they're blockers.
