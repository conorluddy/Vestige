# Vestige V0.5.2 PRD — Menu-bar Controls

## 1. Product Summary

V0.5.1 shipped `Vestige.app` — a read-only macOS menu-bar window over the daemon's
`~/.vestige/daemon.status.json` snapshot (PR #90). It surfaces daemon health and
per-project memory state but cannot *act*: every kick, pause, or config reload still
requires a `vestige daemon …` CLI invocation.

V0.5.2 makes the menu-bar app a **control surface**. It adds one new daemon IPC method
(`daemon.pause`, with a `daemon.resume` companion), wires the existing `daemon.kick` /
`daemon.reload_config` methods into menu actions, and promotes the transient menu popover
to a **persistent project workspace window**. It is the second and final phase of issue
**#88** (VestigeUI) — the mutations explicitly deferred from the V0.5.1 read-only MVP.

## 2. Product Thesis

The daemon already exposes everything an operator needs over a Unix socket; what's missing
is a glanceable, one-click surface for the everyday Mac user who shouldn't have to learn
`launchctl` or memorise `vestige daemon` flags. V0.5.2 closes that gap **without changing
the daemon's contract shape** — it is a thin UI client plus exactly one additive IPC method
and one additive status field. The daemon stays the source of truth; the app stays a client.

## 3. Goals

1. **`daemon.pause` / `daemon.resume` IPC** — suppress future scheduler ticks until a
   caller-supplied timestamp; resume early on demand. Best-effort: in-flight jobs complete.
2. **`paused_until` status field** — additive `Option<String>` on `DaemonStatus` so every
   observer (CLI, menu-bar app) can render pause state. Schema version stays `1`.
3. **`vestige daemon pause` / `vestige daemon resume` CLI subcommands** — headless parity
   and the testable, dogfoodable entry point for the new IPC (mirrors how `kick` has both
   a CLI controller and an IPC method).
4. **Menu-bar actions** — kick embed sweep, pause (1h / until morning), resume, reload
   config, plus convenience shell-outs (open browser, open log, run doctor) and a
   `Daemon: enabled/disabled` toggle that shells out to `daemon install` / `uninstall`.
5. **Persistent project workspace window** — a real window (not just the `MenuBarExtra`
   popover) that keeps per-project detail open while you work.

## 4. Non-Goals

- **No graphical memory browser / editor.** That is V0.10 (Memory dashboard). VestigeUI
  stays a control surface, not a memory editor.
- **No new write path to memory data.** `daemon.pause` only suppresses scheduling. It
  touches no `memories`, `memory_events`, or FTS rows. Soft-delete, project-scope, and
  immutable-migration rules are untouched.
- **Pause does not persist across daemon restarts** (V0.5.2). Pause state is in-memory;
  a launchd `KeepAlive` restart clears it. Persisting pause is an open question (§14).
- **No Linux/Windows GUI.** macOS-only by design; the IPC + CLI half works anywhere the
  daemon runs, but the app is `MenuBarExtra` SwiftUI.
- **No App Store / notarized auto-update pipeline.** Continue the V0.5.1 distribution model
  (unsigned `dist/Vestige.app` via `scripts/build-app.sh`); signing is deferred to v1.0.

## 5. Target User

The macOS Vestige user running the opt-in daemon who wants to pause background work during
a battery-sensitive or focus session, kick a sweep after a burst of memories, or toggle the
daemon on/off — all without dropping to a terminal. The headless/agentic user is served by
the parity CLI subcommands.

## 6. Core Concepts

### 6.1 Pause is tick suppression, not process control
The daemon process keeps running under launchd. `daemon.pause` sets an in-memory
`paused_until: Option<DateTime<Utc>>`. While `now < paused_until`, the scheduler skips the
embed / prune / TTL ticks but **still runs the 5 s status tick** so the status file keeps
refreshing (and surfaces `paused_until`). Once the timestamp passes, ticks resume with no
explicit `resume` needed — pause auto-expires.

### 6.2 Best-effort, tick-boundary semantics
The scheduler already avoids mid-tick interruption by design (the config-reload pattern
breaks the inner `select!` to an `'outer` loop at tick boundaries). Pause reuses that
boundary: any job already running to completion finishes; only the *next* scheduled tick is
suppressed. Documented to callers as "pause is best-effort — in-flight jobs complete."

### 6.3 The app is a client, not a daemon replacement (carried from #88)
Quitting `Vestige.app` stops the UI, never the daemon (launchd `KeepAlive=true`). The
`Daemon: enabled/disabled` toggle shells out to `vestige daemon install` / `uninstall` so
GUI-only users never touch `launchctl`, while the daemon's lifecycle stays decoupled from
the UI process.

## 7. User Experience

### 7.1 Pause from the menu
Click the menu-bar icon → `Pause` submenu → `For 1 hour` / `Until tomorrow morning`. The
icon gains a paused affordance (e.g. a slashed/grey dot), the header shows `paused · resumes
in 47m`, and a `Resume` item appears. `Resume` clears the pause immediately.

### 7.2 Kick / reload from the menu
`Kick embed sweep now` → `daemon.kick {job: "embed"}`. `Reload config` → `daemon.reload_config`.
Both show a transient confirmation (toast or checkmark) and refresh on the next status tick.

### 7.3 Headless parity
```bash
vestige daemon pause --for 1h        # or --until 2026-06-04T08:00:00Z
vestige daemon status                # shows paused_until
vestige daemon resume
```
`pause` without `--for`/`--until` is an error (no implicit default duration). Exactly one of
`--for <dur>` / `--until <rfc3339>` is required.

### 7.4 Persistent workspace window
A `Open Vestige Window` menu item (and ⌘-shortcut) opens a standard window listing projects
with their detail rows, kept open independent of the popover. Closing it returns to
menu-bar-only operation; the daemon is unaffected.

## 8. IPC Surface

Adds two methods to the existing four (`daemon.status`, `daemon.kick`,
`daemon.register_project`, `daemon.reload_config`) dispatched in
`crates/vestige-daemon/src/ipc/methods.rs`.

### 8.1 `daemon.pause`
- **Params**: `{ "until": "<rfc3339>" }` — absolute resume time, UTC. The CLI translates
  `--for <dur>` into `now + dur` before sending; the IPC method only accepts an absolute
  instant (one authoritative shape, no clock-skew ambiguity on the daemon side).
- **Behaviour**: sets `paused_until`, pushes it to the scheduler via a `watch` channel
  (mirrors the `config_tx` reload path). Returns `{ "paused_until": "<rfc3339>" }`.
- **Validation**: `until` in the past → `INVALID_PARAMS` error envelope. Malformed
  timestamp → `INVALID_PARAMS`.

### 8.2 `daemon.resume`
- **Params**: none.
- **Behaviour**: clears `paused_until` (sets `None`), pushes to the scheduler. Returns
  `{ "paused_until": null }`. Resuming when not paused is a no-op success (idempotent).

### 8.3 Error envelope
Reuses the existing JSON-RPC error envelope (`§11.5` of the daemon PRD): standard
`-32602 INVALID_PARAMS`, `-32601 METHOD_NOT_FOUND`, `-32603 INTERNAL_ERROR`.

## 9. Status File Schema

One additive field on `DaemonStatus` (`crates/vestige-daemon/src/ipc/status_file.rs`):

```rust
/// RFC-3339 instant until which scheduled ticks are suppressed, or `None` when
/// the daemon is running normally. Additive field; older readers default to
/// `None`. Auto-clears once the instant passes.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub paused_until: Option<String>,
```

`schema_version` stays **1** — this is additive, matching the established convention
(`memory_count`, `candidate_count`, `last_memory_at` were added the same way in V0.5.1).
The Swift `Codable` mirror tolerates the new field and renders pause state when present.

## 10. CLI Requirements

### 10.1 `vestige daemon pause`
- Flags: `--for <humantime duration>` XOR `--until <rfc3339>` (exactly one required).
- Resolves the absolute instant, opens `~/.vestige/daemon.sock`, sends `daemon.pause`.
- `--json` prints the `{ paused_until }` result; text mode prints `paused until <local time>
  (resumes in <relative>)`.
- Daemon not running → clear error directing the user to `vestige daemon start`.

### 10.2 `vestige daemon resume`
- No flags. Sends `daemon.resume`. `--json` / text symmetry with `pause`.

Both are thin adapters in `crates/vestige-cli/src/commands/daemon/` — parse → socket RPC →
format. No business logic in the CLI (matches the `kick` controller pattern).

## 11. App Requirements (Swift, `app/Vestige-Mac/`)

1. **Socket client** — a minimal `Network.framework` Unix-domain client
   (`NWConnection(to: .unix(path: "~/.vestige/daemon.sock"))`) speaking newline-delimited
   JSON-RPC 2.0. This is net-new: the V0.5.1 MVP was status-file-only (no socket).
2. **Action wiring** in `MenuView.swift` — kick, pause (submenu), resume, reload, plus
   `Process` shell-outs for `daemon install/uninstall`, `vestige browse`, open log, doctor.
3. **Pause affordance** — icon state + header line driven by the new `paused_until` field
   in the `@Observable` status model.
4. **Persistent workspace window** — add a `Window`/`WindowGroup` scene alongside the
   existing `MenuBarExtra`; reuse `ProjectRow` for the list.
5. **Accessibility** — `accessibilityIdentifier` on every actionable control (project
   convention), light/dark review, keyboard shortcuts.

## 12. Architecture

### 12.1 Where the work lands
- **Rust (in the Cargo workspace)**:
  - `crates/vestige-daemon/src/ipc/methods.rs` — two new dispatch arms + handlers; thread a
    `pause_tx: watch::Sender<Option<DateTime<Utc>>>` through `dispatch` (parallel to
    `config_tx`).
  - `crates/vestige-daemon/src/scheduler.rs` — hold a `pause_rx`; at each embed/prune/ttl
    tick boundary, skip the job when `now < paused_until`. The status tick is never skipped
    and writes `paused_until` into the snapshot.
  - `crates/vestige-daemon/src/ipc/status_file.rs` — the additive field.
  - `crates/vestige-cli/src/commands/daemon/` — `pause` + `resume` subcommands.
- **Swift (outside Cargo)**: `app/Vestige-Mac/Vestige/Vestige/` — socket client, action
  wiring, window scene.

### 12.2 Invariants carried forward
- **No new write path.** Pause suppresses scheduling only; no memory mutation.
- **Additive schema only.** `schema_version` unchanged; no field removed/renamed.
- **Daemon adds no new dependency on the app.** The daemon is fully controllable via CLI
  and socket without the app present.

## 13. Testing Requirements

### Integration / unit (Rust, primary line of defence)
1. `daemon.pause` sets `paused_until`; scheduler skips the next embed tick while paused.
2. In-flight job completes — a job already running when pause arrives is not cancelled.
3. Auto-expiry — once `paused_until` passes, the next tick runs without an explicit resume.
4. `daemon.resume` clears pause; ticks resume immediately; resuming when not paused is a
   no-op success.
5. `paused_until` round-trips through `write_atomic` / decode and is absent (not `null`-noise)
   when `None`.
6. Past/ malformed `until` → `INVALID_PARAMS`.
7. CLI smoke: `daemon pause --for` and `--until` are mutually exclusive and one is required;
   `--json` output shape.

### Swift
Light unit coverage on the JSON-RPC client encode/decode and the pause-state view mapping;
manual matrix (macOS 13/14/15, light/dark) for the icon transitions and window.

## 14. Open Questions / Future Work

1. **Persist pause across daemon restarts?** V0.5.2 keeps pause in-memory (lost on launchd
   restart). If users report surprise resumes after a crash-restart, persist `paused_until`
   to a small state file and reload it on boot. Deferred until there's evidence it matters.
2. **Per-project pause.** V0.5.2 pause is daemon-global (all projects, all jobs). Pausing a
   single project or a single job class is plausible later but unscheduled.
3. **Login Item autostart** (`SMAppService`) for the app itself — noted in #88; can ride
   along here or in a follow-up.

## 15. Acceptance Criteria

- `daemon.pause` / `daemon.resume` IPC methods implemented, validated, and unit-tested per
  §13; `paused_until` surfaces in the status file with `schema_version` still `1`.
- `vestige daemon pause --for/--until` and `vestige daemon resume` work headless with text
  and `--json` output.
- The scheduler suppresses embed/prune/ttl ticks while paused, keeps the status tick running,
  and auto-resumes on expiry; in-flight jobs complete.
- `Vestige.app` performs kick, pause, resume, reload, and the daemon enable/disable toggle
  via the socket / shell-outs; quitting the app does not stop the daemon.
- The persistent workspace window opens, lists projects, and is independent of the popover.
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green for the
  Rust changes; `docs/v0.5.md` (or a `docs/v0.5.2.md` walkthrough) documents the controls.

## 16. References

- Issue #88 — VestigeUI (this PRD is its phase-2 / mutations half).
- `docs/prd/vestige_v_0_5_daemon_prd.md` — daemon IPC surface (§11), status schema (§12),
  scheduler architecture (§14).
- `docs/src/data.js` — canonical roadmap (V0.5.2 entry).
- PR #90 — V0.5.1 read-only menu-bar MVP.
