# Vestige V0.5 PRD — Daemon Runtime

## 1. Product Summary

Vestige V0.5 introduces an opt-in per-host daemon — the first long-lived process in the Vestige stack — to handle scheduled maintenance jobs that cannot practicably run inline with each CLI or MCP invocation.

V0–V0.4 proved that a fully one-shot model is clean and safe: every invocation opens SQLite, does its work, and closes. That model works until the work is intrinsically periodic: embed sweeps want to run on a cadence, not only when a user remembers to type `vestige embed --all`. V0.5 adds that background cadence while keeping all one-shot paths intact. The daemon is a co-tenant of the database, not its owner — it shares the WAL with CLI and MCP processes via a 5000 ms `busy_timeout` and calls the same `vestige-engine` and `vestige-store` APIs that the CLI does. No new write paths. No new schema.

The daemon installs as a macOS LaunchAgent, restarts within seconds of a crash, and exposes three observability surfaces: a JSON status file (atomic, no socket required), a log file, and a control socket for mutations.

## 2. Product Thesis

V0.1–V0.4 deferred any work that "needs to happen between commands." That deferral produced two workarounds that are now friction:

1. Semantic search is only as fresh as the last `vestige embed --all` run. Agents know this and work around it; developers forget and get stale recall.
2. Trace eviction and candidate TTL are applied inline, adding latency to unrelated commands.

PRD §8 ("Storage layout") names background indexing, lifecycle scheduling, and concurrency control as canonical daemon duties. V0.5 addresses the first two; the third (concurrency control via daemon-as-single-writer) is deferred to V0.6 pending evidence that WAL contention is actually a problem in practice.

The daemon is explicitly opt-in. Users who prefer the fully one-shot model lose nothing. Users who install the LaunchAgent gain fresh embeddings and bounded trace tables with zero ongoing effort.

## 3. Goals

V0.5 should enable Vestige to:

1. Run a background daemon process invoked via `vestige daemon start` or the macOS LaunchAgent.
2. Perform three recurring job classes: embed sweep, trace prune, and candidate TTL (configurable cadences; TTL off by default).
3. Expose a read-only status surface via `~/.vestige/daemon.status.json` rewritten atomically every 5 seconds.
4. Expose a mutating control surface via `~/.vestige/daemon.sock` (three JSON-RPC 2.0 methods).
5. Install and uninstall as a macOS LaunchAgent (`KeepAlive = true`, `RunAtLoad = true`) via `vestige daemon install` / `vestige daemon uninstall`.
6. Coexist with one-shot CLI/MCP processes via WAL mode and `busy_timeout = 5000 ms`.
7. Obey all existing invariants: soft-delete only, project-scope boundary, immutable migrations, newtype IDs.
8. Add a `[daemon]` config block to `.vestige/config.toml` with documented defaults.
9. Ship without breaking any existing CLI or MCP surface.
10. Pass `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`.

## 4. Non-Goals

V0.5 should not include:

- **MCP-talks-to-daemon RPC.** Agents communicate with the MCP server over stdio as before; the daemon has no MCP surface. Deferred to V0.6.
- **Cross-project federation.** Each worker holds exactly one project's `Store`. Project A cannot see project B's data. Deferred to V0.7 (cross-project query milestone).
- **File-system watcher on `memory.sqlite`.** Timer-driven cadence is sufficient and avoids `notify`/debounce complexity. Revisit in V0.6 if lag is reported.
- **Linux/Windows.** macOS only; `vestige daemon install` emits a clear error on other platforms. Linux systemd user-service in V0.6; Windows in V0.8+.
- **REM consolidation jobs.** Memory rewriting, clustering, and the REM (Review · Evaluate · Merge) consolidation pass shifted to V0.7 after the PRD was reordered. Earlier drafts called this "Dream-Lite".
- **iOS.** No Unix sockets accessible across iOS apps; no LaunchAgents equivalent; no on-device way to run a persistent Rust process against `~/.vestige/`. An iOS surface requires a server-hosted Vestige — V0.8+ and a different product shape.
- **Real embedding provider selection in the daemon.** ~~The daemon defaults to `FakeEmbeddingProvider` in V0.5. Wiring the configured `[embeddings]` provider into the daemon (so workers use the same model as the CLI) lands in V0.6.~~ Delivered in Wave 8 — each worker reads its project's `[embeddings]` config and builds a provider via `vestige_embed::build_provider`, with `FakeEmbeddingProvider` as fallback.

## 5. Target User

Same primary user as V0.4 — solo developer or agent-heavy builder. The specific V0.5 user problems:

> "I just asked Vestige to search for 'auth refactor' after recording three new decisions. It returned nothing. I forgot to run `vestige embed --all` again."

> "My `vestige trace list` is slow. There are 40 000 rows in `query_events`. I know there's a FIFO cap but it only runs when I do a search."

> "I want the daemon to be invisible. Install once, forget it."

V0.5 solves all three. The daemon keeps embeddings current, prunes traces on a daily cadence, and does so with zero user intervention after the initial `vestige daemon install`.

## 6. Core Concepts

### 6.1 Daemon as co-tenant

The daemon opens each project's `memory.sqlite` with WAL mode and `busy_timeout = 5000 ms`. It does not claim exclusive access. CLI and MCP processes read and write the same SQLite database concurrently; WAL allows one writer and many readers simultaneously. The busy_timeout ensures that the daemon backs off gracefully during brief contention windows (e.g. a CLI write mid-embed sweep).

### 6.2 Per-project worker thread

`rusqlite::Connection` is `!Send`: it cannot be shared across threads. The daemon therefore owns one OS worker thread per project; each thread holds exactly one `Store` connection. The scheduler dispatches job commands to workers via `tokio::sync::mpsc` channels.

### 6.3 Registry

`ProjectRegistry` is the in-memory map from `ProjectId` to `ProjectWorker`. At startup it scans `~/.vestige/projects/*/memory.sqlite` and spawns a worker thread for each database found. CLI invocations may call `daemon.register_project` over the control socket to notify the daemon of newly initialised projects without requiring a restart.

### 6.4 Cancellation

The daemon uses `tokio::sync::watch<bool>` for fan-out cancellation. Once `true` is sent on the channel, every task that was waiting or will start waiting sees it immediately. `SIGTERM` and `SIGINT` both trigger cancellation; `SIGKILL` leaves a stale pidfile that the next start cleans up.

### 6.5 Status file

`~/.vestige/daemon.status.json` is the read-only observability surface. The scheduler writes it atomically every 5 seconds (write-to-`.tmp` + POSIX `rename`). Readers — `vestige daemon status`, the future Swift menu-bar app — read this file without holding any lock. Absence of the file means the daemon is not running.

### 6.6 Control socket

`~/.vestige/daemon.sock` is a Unix-domain socket accepting newline-delimited JSON-RPC 2.0. One request per connection; the server closes the connection after writing the response. Four methods: `daemon.status`, `daemon.kick`, `daemon.register_project`, `daemon.reload_config`.

## 7. User Experience

### 7.1 Install and first run

```bash
vestige daemon install      # writes plist, calls launchctl load -w
vestige daemon status       # shows pid, uptime, supervised projects
```

The daemon starts immediately after `install`. On the next login it starts automatically via launchd. On crash, launchd restarts it within 10 seconds.

### 7.2 Day-to-day

The daemon is invisible during normal use. Embeddings stay current. Trace tables stay bounded. The user notices it only through `vestige status` (daemon line) or `vestige daemon status`.

### 7.3 Manual control

```bash
vestige daemon kick embed           # force embed sweep across all projects
vestige daemon kick embed --project proj_vestige   # one project only
vestige daemon kick prune           # force trace prune now
vestige daemon kick ttl             # force candidate TTL now
vestige daemon stop                 # SIGTERM + wait
vestige daemon restart              # stop + launchctl kickstart -k (Wave 8)
vestige daemon doctor               # 8-check health diagnostic (Wave 8)
vestige daemon log -f               # follow the log
```

### 7.4 Teardown

```bash
vestige daemon uninstall    # launchctl unload -w, remove plist
```

The `stop` command and uninstall are independent. `stop` sends SIGTERM to the running process; `uninstall` removes the LaunchAgent registration. Running `uninstall` while the daemon is live stops it and removes the plist.

## 8. Data Model

V0.5 adds no schema migration. The daemon reads and writes the same tables that the CLI and MCP server use:

- `memories`, `memory_representations`, `memory_embeddings` — updated by the embed sweep.
- `query_events` — pruned by the trace-prune job.
- `candidates` — marked rejected by the candidate-TTL job.
- `memory_events` — append-only journal, never modified by the daemon.

No new tables. No new views. `Store::open` runs migrations idempotently on every connection; the daemon benefits automatically when new migrations ship.

## 9. CLI Requirements

### 9.1 `vestige daemon start`

```
vestige daemon start [--foreground] [--detach]
```

Starts the daemon. `--foreground` is the default and is required under launchd. `--detach` double-forks to background but is not yet implemented in V0.5 — the command exits with a clear error directing the user to `vestige daemon install` instead.

### 9.2 `vestige daemon stop`

```
vestige daemon stop [--timeout <secs>] [--json]
```

Reads the pidfile, sends SIGTERM, polls every 100 ms until exit or timeout (default 10 s). Idempotent: if the pidfile is absent or the process is already gone, exits 0.

### 9.3 `vestige daemon status`

```
vestige daemon status [--json] [--watch]
```

Reads `~/.vestige/daemon.status.json` and formats output. Never connects to the control socket — this means it works even when the socket is down or the daemon is not running (prints `daemon: not running`). `--watch` refreshes every 5 seconds. Must return within 100 ms.

### 9.4 `vestige daemon kick`

```
vestige daemon kick {embed|prune|ttl} [--project <id>] [--json]
```

Sends `daemon.kick` over the control socket. `--project` restricts the kick to one project ID; omitting it kicks all registered projects. Prints the `projects_queued` count from the response.

### 9.5 `vestige daemon log`

```
vestige daemon log [-f] [-n <lines>]
```

Prints the last `<lines>` lines (default 100) from `~/.vestige/daemon.log`. `-f` follows the log via `tail -F` (handles log rotation). Exits with a clear error if the log file is absent.

### 9.6 `vestige daemon install`

```
vestige daemon install [--force] [--no-load] [--bin <path>] [--json]
```

macOS only. Renders the plist template (embedded via `include_str!` at compile time) with the resolved vestige binary path and home directory. Writes to `~/Library/LaunchAgents/com.vestige.daemon.plist`. Calls `launchctl load -w` unless `--no-load` is set. `--force` overwrites an existing plist. `--bin` overrides the binary path (useful after Homebrew upgrades or when the running binary is not the one to install).

### 9.7 `vestige daemon uninstall`

```
vestige daemon uninstall [--no-unload] [--if-exists] [--json]
```

macOS only. Calls `launchctl unload -w` on the plist (unless `--no-unload`) then removes the plist file. Non-zero `launchctl unload` exit is a warning, not a hard error — the plist is removed regardless. `--if-exists` suppresses the error when the plist is already absent.

### 9.8 `vestige daemon restart` (Wave 8)

```
vestige daemon restart [--json]
```

macOS only. Sends SIGTERM and waits for the daemon to stop (up to 10 s), then calls `launchctl kickstart -k gui/$UID/com.vestige.daemon` to start a fresh instance under launchd. Exits with error if the LaunchAgent plist is not installed. Prints the new PID on success.

### 9.9 `vestige daemon doctor` (Wave 8)

```
vestige daemon doctor [--json]
```

Runs 8 health checks and prints a pass/fail summary. Checks include: LaunchAgent plist present, plist binary path matches current binary, daemon process running, status file fresh (< 30 s old), socket accepting connections, at least one project registered, all project DBs readable, and no stale pidfile from a dead process. Exits 0 if all checks pass; exits 1 with a structured list of failures otherwise.

## 10. MCP Requirements

**None.** The daemon has no MCP surface in V0.5. The MCP contract is unchanged. Agents communicate with `vestige mcp` over stdio as before.

The existing `vestige_bootstrap` tool is unaffected. Agents detect daemon presence via `vestige status` (the daemon line) or by reading `~/.vestige/daemon.status.json` — no new MCP tool is needed for this in V0.5.

## 11. IPC Surface

Four JSON-RPC 2.0 methods over `~/.vestige/daemon.sock`. Newline-delimited framing. One request per connection; the server closes the connection after writing the response.

### 11.1 `daemon.status`

Returns the current `DaemonStatus` snapshot.

```
→ {"jsonrpc":"2.0","id":1,"method":"daemon.status","params":{}}
← {"jsonrpc":"2.0","id":1,"result":{"schema_version":1,"version":"0.5.0","pid":12345,...}}
```

Params: `{}` (none required).
Result: `DaemonStatus` object (see §12).

### 11.2 `daemon.kick`

Runs a job immediately across all or one project.

```
→ {"jsonrpc":"2.0","id":2,"method":"daemon.kick","params":{"job":"embed"}}
← {"jsonrpc":"2.0","id":2,"result":{"queued":true,"queued_at":"2026-05-19T09:00:00Z","projects_queued":2}}

→ {"jsonrpc":"2.0","id":3,"method":"daemon.kick","params":{"job":"prune","project_id":"proj_vestige"}}
← {"jsonrpc":"2.0","id":3,"result":{"queued":true,"queued_at":"2026-05-19T09:00:05Z","projects_queued":1}}
```

Params:
- `job` (required): `"embed"` | `"prune"` | `"ttl"`
- `project_id` (optional): restrict to one project. Must parse as a valid `ProjectId` (`proj_` prefix). If present and not in the registry, returns error code `-32000` with `code: "PROJECT_NOT_REGISTERED"`.

Result:
- `queued`: always `true` in V0.5 (field reserved for async-queue semantics in V0.6).
- `queued_at`: RFC3339 timestamp when the kick was processed.
- `projects_queued`: number of projects that ran (or attempted) the job.

### 11.3 `daemon.register_project`

Upserts a project into the in-memory registry without requiring a daemon restart.

```
→ {"jsonrpc":"2.0","id":4,"method":"daemon.register_project",
    "params":{"project_id":"proj_my-app","project_name":"My App","repo_root":"/Users/conor/my-app"}}
← {"jsonrpc":"2.0","id":4,"result":{"registered":true,"project_id":"proj_my-app"}}
```

Called by CLI invocations after `vestige init` to notify the daemon of a newly created project. Idempotent: calling it on an already-registered project returns `registered: false` with no other effect.

Params: `project_id` (required, `proj_` prefixed), `project_name` (required), `repo_root` (required, absolute path).
Result: `registered` (bool — true on new insert, false on no-op), `project_id` (echoed).

### 11.4 `daemon.reload_config`

Reloads the `[daemon]` cadence configuration from each project's `.vestige/config.toml` without requiring a daemon restart. Scoped to cadence fields only — `embed_sweep_interval_secs`, `trace_prune_interval_secs`, `candidate_ttl_days`, `candidate_ttl_sweep_interval_secs`. Provider changes (`[embeddings]`) still require a restart.

```
→ {"jsonrpc":"2.0","id":5,"method":"daemon.reload_config","params":{}}
← {"jsonrpc":"2.0","id":5,"result":{"reloaded":true,"projects_reloaded":2}}
```

Params: `{}` (none required).
Result: `reloaded` (always `true`), `projects_reloaded` (count of projects whose config was re-read).

### 11.5 Error envelope

All method errors return the standard JSON-RPC 2.0 error object with a `data` field for machine-readable details:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32000,
    "message": "project not registered",
    "data": {
      "code": "PROJECT_NOT_REGISTERED",
      "message": "project proj_nonexistent is not in the daemon registry",
      "retryable": false
    }
  }
}
```

Standard JSON-RPC 2.0 protocol error codes:
- `-32700` parse error (framing failed before a valid request was parsed)
- `-32600` invalid request (jsonrpc field was not "2.0")
- `-32601` method not found
- `-32602` invalid params
- `-32603` internal error

Server-defined application errors use code `-32000`. The `data.code` field carries a machine-readable error tag (e.g. `PROJECT_NOT_REGISTERED`). `data.retryable` is `false` for hard failures and `true` for transient contention errors.

## 12. Status File Schema

`DaemonStatus` is the contract for `vestige daemon status --json` and the future Swift menu-bar app. Evolve additively — new fields are fine; removing or renaming existing fields is a breaking change.

Source of truth: `crates/vestige-daemon/src/ipc/status_file.rs`.

### `DaemonStatus`

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | `u32` | Schema version; starts at 1, bumped only on breaking changes. |
| `version` | `String` | `vestige` crate version at build time (`CARGO_PKG_VERSION`). |
| `pid` | `u32` | OS PID of the running daemon process. |
| `started_at` | `String` | RFC3339 timestamp when this daemon process started. |
| `uptime_secs` | `u64` | Whole-second uptime since `started_at`. |
| `projects` | `Vec<ProjectStatus>` | One entry per discovered or registered project. |
| `next_jobs` | `Vec<ScheduledJob>` | Next-scheduled jobs across all projects, ordered by `at` ascending. Populated via scheduler-tracked `TickState` (shipped in Wave 8). |

### `ProjectStatus`

| Field | Type | Meaning |
|---|---|---|
| `project_id` | `ProjectId` | Validated project ID (`proj_<slug-or-ULID>`). |
| `project_name` | `String` | Human-readable project name from `.vestige/config.toml`. |
| `repo_root` | `String` | Absolute path to the repository root. |
| `last_embed_run` | `Option<String>` | RFC3339 timestamp of last embed sweep, or `null` if never run. |
| `last_prune_run` | `Option<String>` | RFC3339 timestamp of last trace-prune job, or `null` if never run. |
| `last_ttl_run` | `Option<String>` | RFC3339 timestamp of last candidate-TTL sweep, or `null` if never run. |
| `pending_embeds` | `u64` | Count of memory representations awaiting embedding. |

### `ScheduledJob`

| Field | Type | Meaning |
|---|---|---|
| `kind` | `JobKind` | `"embed"` \| `"prune"` \| `"candidate_ttl"` |
| `project_id` | `ProjectId` | Project this job runs against. |
| `at` | `String` | RFC3339 timestamp of the next scheduled execution. |

## 13. Config Schema

The `[daemon]` block is optional in `.vestige/config.toml`. Omitting the block leaves all defaults in effect.

Source of truth: `crates/vestige-config/src/schema.rs`, `DaemonConfig` struct and `DAEMON_DEFAULT_*` constants.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | `bool` | `false` | Master switch. Opt-in; `true` after the user runs `vestige daemon install`. |
| `embed_sweep_interval_secs` | `u64` | `600` | Embed sweep cadence in seconds (10 minutes). |
| `trace_prune_interval_secs` | `u64` | `86400` | Trace VACUUM cadence in seconds (24 hours). |
| `candidate_ttl_days` | `u32` | `0` | Candidates older than this many days are marked expired. `0` disables the TTL. |
| `candidate_ttl_sweep_interval_secs` | `u64` | `3600` | How often to run the TTL check in seconds (1 hour). Only meaningful when `candidate_ttl_days > 0`. |
| `log_level` | `String` | `"info"` | Tracing log level for the daemon process (`error` \| `warn` \| `info` \| `debug` \| `trace`). |
| `socket_path` | `Option<String>` | `~/.vestige/daemon.sock` | Override the Unix socket path. Mainly for tests. |
| `status_file_path` | `Option<String>` | `~/.vestige/daemon.status.json` | Override the status file path. Mainly for tests. |

Example `.vestige/config.toml` with daemon section:

```toml
project_id   = "proj_vestige"
project_name = "Vestige"

[daemon]
embed_sweep_interval_secs = 300   # 5 minutes — faster embeds on an active project
candidate_ttl_days        = 14    # expire candidates after two weeks
```

## 14. Architecture

### 14.1 Process topology

One daemon per host. Users who work across multiple machines each have their own daemon instance — cross-machine sync is out of scope.

The daemon is started via:

1. **`vestige daemon start`** — foreground, blocks the terminal. Used directly when testing.
2. **macOS LaunchAgent** (`com.vestige.daemon.plist`) — `RunAtLoad = true`, `KeepAlive = true`, `Nice = 10`, `ProgramArguments = [<vestige>, daemon, start, --foreground]`. Stdout and stderr both route to `~/.vestige/daemon.log`. launchd restarts the daemon within seconds of a crash.

Single-instance enforcement: `~/.vestige/daemon.pid` with `fcntl` advisory lock. If the lock cannot be acquired, the new process exits with `daemon already running pid=<N>`.

### 14.2 Crate structure

New crate **`vestige-daemon`** added between `cli` and `engine`. Dep order remains strictly one-way:

```
cli ─┬──→ daemon ─┐
     ├──→ engine ─┤
     ├──→ mcp ────┤
     └────────────┴──→ store ──→ core
                         ↑
embed ───────────────────┘
config ──→ core
```

`vestige-daemon` may depend on `vestige-engine`, `vestige-store`, `vestige-config`, `vestige-core`, and `vestige-embed`. It must not depend on `vestige-cli` or `vestige-mcp`.

### 14.3 Concurrency model

`rusqlite::Connection` is `!Send`. The daemon therefore uses one OS worker thread per project, each owning exactly one `Store`. The tokio scheduler communicates with each worker via `tokio::sync::mpsc`; workers do blocking SQLite calls in their own thread and send results back on `oneshot` channels.

The `tokio::sync::Mutex<ProjectRegistry>` is held only for the duration of each registry read or mutation — never across an `await` that does I/O. This minimises contention between the scheduler and the IPC server, which both hold references to the same registry.

### 14.4 WAL coexistence and busy_timeout

Every daemon `Store::open` call sets `PRAGMA busy_timeout = 5000`. When a CLI or MCP process is mid-write, the daemon's write attempt blocks up to 5 seconds before returning `SQLITE_BUSY`. This is the same timeout used across all writer paths and is sufficient for the lock durations Vestige generates (sub-millisecond for most writes; sub-second even for migrations on first-run).

WAL mode allows one writer and many readers simultaneously. Embed sweep reads far more than it writes; trace prune is a writer but runs daily. Contention is expected to be rare in practice.

### 14.5 Invariants that carry forward

All hard rules from the core codebase apply to daemon writes without exception:

- **Soft-delete only.** The daemon never issues `DELETE FROM memories`. Candidate TTL marks rejected with `reason='expired'`; trace prune evicts rows from `query_events` (traces are not memory) within the configured FIFO cap.
- **Project scope.** Each worker holds exactly one `Store`. A worker for project A literally cannot see project B's database.
- **Immutable migrations.** `Store::open` runs the migration runner idempotently. The daemon adds no new migration mechanism.
- **MCP intent-not-mechanics.** The control socket exposes three semantic verbs, not raw SQL.
- **Newtype IDs.** `ProjectId` is parsed and validated before use; bare strings never reach storage.

## 15. Scheduling Cadences

| Job | Default cadence | Trigger | Config field |
|---|---|---|---|
| Embed sweep | 600 s (10 min) | tokio interval + on-demand `daemon.kick {job:"embed"}` | `embed_sweep_interval_secs` |
| Trace prune | 86400 s (24 h) | tokio interval + on-demand `daemon.kick {job:"prune"}` | `trace_prune_interval_secs` |
| Candidate TTL | 3600 s (1 h) if `candidate_ttl_days > 0` | tokio interval + on-demand `daemon.kick {job:"ttl"}` | `candidate_ttl_sweep_interval_secs` |
| Status file write | 5 s (hardcoded) | tokio interval | not configurable |

The first embed, prune, and TTL ticks are skipped at startup so the daemon does not run full sweeps on every restart. The status tick fires immediately on first poll so the status file exists as soon as the daemon is up.

The scheduler uses a biased `tokio::select!` with the cancellation check as the highest-priority arm. This guarantees prompt exit on SIGTERM even when multiple ticks are ready simultaneously.

## 16. Implementation Plan

The implementation was structured in eight waves to enable parallel agent work:

| Wave | Tasks | Key files |
|---|---|---|
| 1 | `vestige-daemon` crate scaffold, `DaemonOpts`, `DaemonError`, workspace wiring, README/CLAUDE.md updates | `crates/vestige-daemon/`, `Cargo.toml` |
| 2 | Lifecycle: pidfile + flock, SIGTERM/SIGINT, signal-driven cancellation | `lifecycle.rs` |
| 3 | Per-project workers, `ProjectRegistry`, embed scheduler, status file | `workers.rs`, `registry.rs`, `scheduler.rs`, `ipc/status_file.rs` |
| 4 | IPC server: Unix socket accept loop, `daemon.status` / `daemon.kick` / `daemon.register_project` | `ipc/server.rs`, `ipc/methods.rs` |
| 5 | Trace-prune job and candidate-TTL job | `jobs/trace_prune.rs`, `jobs/candidate_ttl.rs` |
| 6 | LaunchAgent plist, `install` and `uninstall` CLI subcommands, `log` subcommand | `plist.rs`, `commands/daemon/{install,uninstall,log}.rs` |
| 7 | `stop` and `kick` CLI subcommands, `vestige status` daemon detection | `commands/daemon/{stop,kick}.rs`, `commands/status.rs` |
| 8 | Polish and harden — `daemon restart` subcommand, `daemon doctor` 8-check diagnostic, `daemon.reload_config` IPC method, `next_jobs[]` population via `TickState`, `vestige init` live registration, per-project provider selection | `commands/daemon/{restart,doctor}.rs`, `ipc/methods.rs`, `scheduler.rs`, `registry.rs`, `embed/provider.rs` |

**Wave 8 — Polish and harden** (Sonnet/Haiku tier; ~7 task units across 3 dependency-aware sub-waves). Closes the V0.6-candidate items the original PRD anticipated:

- `vestige daemon restart` subcommand (`launchctl kickstart -k gui/$UID/com.vestige.daemon`)
- `vestige daemon doctor` 8-check diagnostic subcommand
- `daemon.reload_config` IPC method (fourth method, scoped to cadence reload only — provider changes still require restart)
- `next_jobs[]` populated in the status file via scheduler-tracked `TickState`
- `vestige init` fires `daemon.register_project` over the socket for live discovery (best-effort, 500 ms timeout, never fails init)
- Per-project provider selection — each worker reads its project's `[embeddings]` config and builds via `vestige_embed::build_provider`, with `FakeEmbeddingProvider` fallback

Verified live on `feat/v0.5-daemon` against 9 supervised projects.

T17 (subprocess test harness) runs in parallel with Wave 7 and owns `crates/vestige-cli/tests/daemon_smoke.rs`.

T18 (this document) runs in parallel with T17.

## 17. Testing Requirements

### Integration tests (primary line of defence)

- `crates/vestige-daemon/tests/lifecycle.rs` — spawn daemon in a TempDir (pointing `projects_root` at an empty dir to isolate from real DBs), send cancellation, assert clean exit and pidfile cleanup.
- `crates/vestige-daemon/tests/embed_sweep.rs` — seed a project DB with un-embedded memories, run the daemon for one tick, assert embeddings appeared.
- `crates/vestige-daemon/tests/ipc_integration.rs` — connect to the socket, call each method, assert envelope shape and structured error on bad params.

### Unit tests

- `crates/vestige-daemon/src/ipc/methods.rs` `#[cfg(test)]` — all four `dispatch_*` paths, including `PROJECT_NOT_REGISTERED` on an unknown project ID.
- `crates/vestige-config/src/schema.rs` — `daemon_config_for(None)` returns documented defaults; partial overrides apply correctly; round-trip through TOML.

### CLI smoke tests

- `crates/vestige-cli/tests/daemon_smoke.rs` (T17) — subprocess harness: `install --no-load`, `start --foreground` (with cancellation), `status --json`, `stop`, `uninstall --if-exists`.

### Invariants to exercise

1. Soft-delete excludes from search after daemon processes a project (no daemon writes break FTS trigger sync).
2. Project-scope boundary: daemon worker for project A cannot affect project B.
3. `vestige daemon status` returns valid JSON within 100 ms when the daemon is not running (reads absent status file, returns `{"running":false}`).
4. Daemon survives SIGKILL and restarts cleanly with pidfile reclaim.
5. `daemon.register_project` is idempotent.
6. Embed sweep is idempotent — running it twice against an already-embedded project produces zero new embeddings.

## 18. Acceptance Criteria

V0.5 is complete when:

1. `vestige daemon install` writes a valid plist to `~/Library/LaunchAgents/com.vestige.daemon.plist` and the daemon starts via launchd. After a `kill -9 <pid>`, launchd restarts the daemon within 10 seconds.
2. `vestige daemon status --json` returns valid JSON with `{"running":false}` when the daemon is down and a full `DaemonStatus` object when it is up, in both cases within 100 ms.
3. From a cold install on a host with N projects (each with un-embedded memories), all embeddings are up to date within `embed_sweep_interval_secs` of the daemon starting, with zero user actions.
4. The daemon can be killed mid-sweep and restarted; the DB is in a consistent state on restart (no orphan rows, no migration drift); the sweep resumes idempotently.
5. Soft-delete, project-scope boundary, and migration immutability all hold under daemon writes.
6. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace` is green.

## 19. Open Questions / Future Work

1. **MCP-talks-to-daemon protocol design (V0.6).** PRD §20 named this but the design was deferred pending V0.5 operational experience. The most likely shape: a new `vestige_daemon_status` MCP tool that reads the status file, and a `vestige_kick` tool that calls `daemon.kick` over the socket.

2. **Linux systemd user service (V0.6).** `vestige daemon install` exits with a clear error on Linux. The systemd unit file can reuse the same plist rendering pattern (`include_str!` template), with a different renderer in `plist.rs`. The `--user` flag in the plan reserves this path.

3. **macOS menu-bar Swift app (`Vestige.app`).** Phase 2 stretch goal for V0.5.x. Reads `daemon.status.json` via `DispatchSource.makeFileSystemObjectSource`; sends mutations to `daemon.sock` via `Network.framework`. Estimated ~300 lines for the MVP. Lives in `app/Vestige-Mac/` (separate SwiftUI project, not built by Cargo). Deferred until the daemon core is stable.

4. ~~Real provider selection in the daemon~~ — **closed in Wave 8** (per-project provider via `vestige_embed::build_provider` reading each project's `[embeddings]` config; `FakeEmbeddingProvider` fallback when config is absent or provider unavailable).

## 20. References

- **Source plan**: `~/.claude/plans/expressive-foraging-newell.md` — the design document this PRD is derived from, covering the five design questions (when/how/who/CLI/Swift), crate layout, IPC contract, and phased implementation.
- **Root PRD**: `docs/prd/vestige_prd.md` — §8 (storage layout), §18.1 (milestone list), §20 (daemon runtime stub that V0.5 fulfils).
- **V0.4 PRD**: `docs/prd/vestige_v_0_4_browser_prd.md` — sibling milestone style; the V0.4 §15 acceptance criteria format is the template for §18 above.
- **V0.3 PRD**: `docs/prd/vestige_v_0_3_provenance_prd.md` — trace storage design that the trace-prune job operates on.
- **V0.5 walkthrough**: `docs/v0.5.md` — user-facing tour; the authoritative reference for command spellings and user-visible behaviour.
