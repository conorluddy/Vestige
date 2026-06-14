---
name: vestige-scan-sessions
description: 'Mine recent local coding-agent transcripts for memories worth keeping, using the `vestige_scan_sessions` MCP tool. Fire at session start (to catch up on what happened in prior sessions), when the user says "scan my sessions", "what did we work on recently?", "catch up from my transcripts", "ingest recent sessions", "mine my history", or when picking up a project after a pause. The tool hands you redacted, cursor-advanced turns from this project''s Claude Code and Codex transcripts; you extract decisions/notes/preferences/questions inline and file each via `vestige_propose_candidate`. Opt-in — requires `[mcp] allow_scan_sessions = true` in `.vestige/config.toml`. Candidates land in the assimilation inbox for human review, never as durable memories directly.'
---

# Scan local sessions for memory candidates

The **agent-driven, zero-config** ingestion mode. You — the agent already in the loop —
do the extraction. No extra model, no API key. The MCP tool only *retrieves* redacted
turns; turning them into candidates is your job.

## When to fire

- **At session start**, to assimilate what happened in prior sessions before doing new work.
- When the user asks to **catch up from / mine / ingest** recent transcripts.
- After a pause on a project, to recover decisions and open questions that were never captured.

Off by default. If the tool returns `SCAN_DISABLED`, tell the user to set
`[mcp] allow_scan_sessions = true` in `.vestige/config.toml` — don't retry blindly.

## How to invoke

Call the MCP tool `vestige_scan_sessions`:

```jsonc
// arguments
{ "max_turns": 100 }   // optional token-budget cap; default 100
```

The read **advances per-file cursors**, so a repeat call surfaces only turns you
haven't seen. It is project-scoped — only this project's sessions are returned.
Each turn carries a `source` (`claude_code` or `codex`) so you can tell where it
came from.

## Response shape

```json
{
  "turns": [
    { "source": "claude_code", "session_id": "…", "role": "user",
      "text": "…redacted…", "line": 42, "source_ref": "claude_code:…:L42" }
  ],
  "sessions_scanned": 3,
  "turns_returned": 100,
  "cursor_advanced": true
}
```

`cursor_advanced: false` with no turns means nothing new — you're caught up. If
`turns_returned == max_turns` there is likely more; call again to drain the backlog.

## After invocation — extract and propose

Read the turns and pull out anything a future session would want:

- **decisions** ("we'll go with X over Y") → `type: "decision"`
- **preferences** ("always…", "never…", "I prefer…") → `type: "preference"`
- **open questions** ("TBD", "unclear whether…") → `type: "question"`
- **notes / gotchas / TILs / workarounds** → `type: "note"`

For each, **dedup first** with `vestige_search` (or `vestige-recall`) so you don't
double-file something already in memory. Then call `vestige_propose_candidate`:

```jsonc
{
  "type": "decision",
  "body": "Settled on rusqlite bundled SQLite with FTS5 over a client/server DB.",
  "rationale": "Local-first, zero external deps; FTS5 covers lexical recall.",
  "source": {
    "type": "session_log",
    "ref": "claude_code:01J…:L42",   // copy the turn's source_ref verbatim
    "content": "…the relevant snippet…"
  }
}
```

Always set `source.type` to `"session_log"` and `source.ref` to the turn's
`source_ref` so provenance is traceable back to the transcript line.

## Idempotence & etiquette

- Safe to re-run — the cursor guarantees you won't re-offer seen turns.
- Candidates are **proposals**, not durable memories. They sit in the inbox until a
  human approves them. Don't over-propose: file the few things genuinely worth keeping,
  not every turn.
- Secrets are already redacted in the returned text, but never paste raw tokens you
  might reconstruct — keep `content` snippets short and high-signal.
