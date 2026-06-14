//! `vestige_scan_sessions` tool — hands the calling agent a batch of redacted,
//! normalised, cursor-advanced turns from local coding-agent transcripts so the
//! agent can extract candidates inline and file them via `vestige_propose_candidate`.
//!
//! This is the **agent-driven** (zero-config) ingestion mode (PRD V0.5.3): no extra
//! model, no API key — "whatever agent you use day-to-day" does the extraction.
//!
//! Gated by `mcp.allow_scan_sessions` (off by default — passive transcript scanning
//! is an explicit opt-in). The tool is read-only w.r.t. memories/candidates; it only
//! advances per-file scan cursors so a re-call surfaces nothing already seen.

use std::path::PathBuf;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router, ErrorData,
};
use serde::{Deserialize, Serialize};

use vestige_core::redact_secrets;
use vestige_engine::{ClaudeCodeSource, IngestError, SessionSource};
use vestige_store::Store;

use crate::server::{err, ok_json, VestigeServer};

// === INPUT SCHEMA ===

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScanSessionsParams {
    /// Max turns to return this call — a token-budget guard. Default 100.
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
}

fn default_max_turns() -> usize {
    100
}

// === OUTPUT SHAPE ===

#[derive(Debug, Serialize)]
struct ScanSessionsResponse {
    /// Redacted turns, in discovery order, capped at `max_turns`.
    turns: Vec<ScanTurnJson>,
    /// Number of in-scope sessions inspected this call.
    sessions_scanned: usize,
    /// Convenience count == `turns.len()`.
    turns_returned: usize,
    /// `false` when an idempotent re-call surfaced nothing new (no cursor moved).
    cursor_advanced: bool,
}

#[derive(Debug, Serialize)]
struct ScanTurnJson {
    /// Source adapter that produced this turn, e.g. `"claude_code"`.
    source: &'static str,
    /// Session identifier (file stem / UUID).
    session_id: String,
    /// `"user" | "assistant" | "system" | …`.
    role: String,
    /// Redacted plain-text content (secrets scrubbed via `redact_secrets`).
    text: String,
    /// 1-based line number within the source transcript.
    line: usize,
    /// Ready-to-use provenance ref for `vestige_propose_candidate`'s `source.ref`,
    /// e.g. `"claude_code:<session_id>:L<line>"`.
    source_ref: String,
}

// === TOOL ROUTER ===

#[tool_router(router = scan_sessions_router, vis = "pub(crate)")]
impl VestigeServer {
    #[tool(
        description = "Return a batch of redacted, normalised turns from this project's local \
                       coding-agent transcripts (Claude Code) for the calling agent to mine. \
                       Extract decisions/notes/preferences/questions inline, then file each via \
                       vestige_propose_candidate with source.type = \"session_log\" and \
                       source.ref = the turn's source_ref. The read advances per-file cursors, so \
                       a repeat call surfaces only new turns. Project-scoped: only this project's \
                       sessions are returned. Disabled unless mcp.allow_scan_sessions = true."
    )]
    pub async fn vestige_scan_sessions(
        &self,
        Parameters(p): Parameters<ScanSessionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let inner = self.inner.lock().await;

        // Advancing scan cursors is a DB write, so honour read-only like the other
        // mutating tools (propose_candidate, record_*).
        if inner.read_only {
            return Err(err(
                "READ_ONLY",
                "MCP server is read-only; vestige_scan_sessions is disabled",
                false,
            ));
        }

        if !inner.config.mcp.allow_scan_sessions {
            return Err(err(
                "SCAN_DISABLED",
                "session scanning is off; set [mcp] allow_scan_sessions = true in .vestige/config.toml",
                false,
            ));
        }

        let sources = build_sources().map_err(map_ingest_error)?;
        let response = collect_batch(&sources, &inner.store, &inner.project_id, p.max_turns)?;
        ok_json(&response)
    }
}

// === PRIVATE HELPERS ===

/// Drain a batch of redacted turns from `sources`, scoped to `project_id`, advancing
/// per-file scan cursors as it goes. Pure over its inputs (no env, no `Inner`), so the
/// batching / cursor / redaction / scope logic is unit-testable against a fake source.
fn collect_batch(
    sources: &[Box<dyn SessionSource>],
    store: &Store,
    project_id: &vestige_core::ProjectId,
    max_turns: usize,
) -> Result<ScanSessionsResponse, ErrorData> {
    let mut turns: Vec<ScanTurnJson> = Vec::new();
    let mut sessions_scanned = 0usize;
    let mut cursor_advanced = false;

    'outer: for source in sources {
        let source_name = source.source_name();

        let discovered = source.discover().map_err(map_ingest_error)?;
        for session in discovered {
            // Project-scope boundary: never surface another project's sessions.
            if &session.project_id != project_id {
                continue;
            }
            sessions_scanned += 1;

            let file_path_str = session.file_path.to_string_lossy().to_string();
            let from = store
                .get_scan_cursor(source_name, &file_path_str)
                .map_err(|e| err("STORE_FAILED", e.to_string(), true))?
                .map(|c| c.last_offset.max(0) as usize)
                .unwrap_or(0);

            let read = source
                .read_turns(&session, from)
                .map_err(map_ingest_error)?;

            // Highest line actually *returned* — we only advance the cursor to here,
            // so turns dropped by the max_turns budget resurface on the next call
            // and idempotency stays honest.
            let mut last_line_included: Option<usize> = None;

            for turn in read {
                let text = redact_secrets(&turn.text);
                if text.trim().is_empty() {
                    // Skip empty / tool-only turns, but still let the cursor move
                    // past them so we don't re-scan them forever.
                    last_line_included = Some(turn.line);
                    continue;
                }

                if turns.len() >= max_turns {
                    // Budget exhausted — stop before consuming this turn so it is
                    // re-offered next call. Persist progress for this session first.
                    if advance_cursor(
                        store,
                        source_name,
                        &file_path_str,
                        &session,
                        from,
                        last_line_included,
                    )? {
                        cursor_advanced = true;
                    }
                    break 'outer;
                }

                let source_ref = format!("{source_name}:{}:L{}", session.session_id, turn.line);
                turns.push(ScanTurnJson {
                    source: source_name,
                    session_id: session.session_id.clone(),
                    role: turn.role,
                    text,
                    line: turn.line,
                    source_ref,
                });
                last_line_included = Some(turn.line);
            }

            if advance_cursor(
                store,
                source_name,
                &file_path_str,
                &session,
                from,
                last_line_included,
            )? {
                cursor_advanced = true;
            }
        }
    }

    let turns_returned = turns.len();
    Ok(ScanSessionsResponse {
        turns,
        sessions_scanned,
        turns_returned,
        cursor_advanced,
    })
}

/// Build the configured session sources. Honours `VESTIGE_CLAUDE_ROOT` as a test
/// seam so the harness can point the Claude Code adapter at a tempdir.
///
/// Codex is added here once #105 lands (integration step C).
fn build_sources() -> Result<Vec<Box<dyn SessionSource>>, IngestError> {
    let claude = match std::env::var_os("VESTIGE_CLAUDE_ROOT") {
        Some(root) => ClaudeCodeSource::with_root(PathBuf::from(root)),
        None => ClaudeCodeSource::new()?,
    };
    Ok(vec![Box::new(claude)])
}

/// Record the scan cursor for a session if the watermark moved forward.
/// Returns `true` when a cursor was advanced.
fn advance_cursor(
    store: &Store,
    source_name: &str,
    file_path_str: &str,
    session: &vestige_engine::DiscoveredSession,
    from: usize,
    last_line_included: Option<usize>,
) -> Result<bool, ErrorData> {
    let Some(new_offset) = last_line_included else {
        return Ok(false);
    };
    if new_offset <= from {
        return Ok(false);
    }
    store
        .record_scan_cursor(
            source_name,
            file_path_str,
            &session.project_id,
            new_offset as i64,
        )
        .map_err(|e| err("STORE_FAILED", e.to_string(), true))?;
    Ok(true)
}

fn map_ingest_error(e: IngestError) -> ErrorData {
    match e {
        IngestError::Io(e) => err("SCAN_IO", e.to_string(), true),
        IngestError::Json(e) => err("SCAN_PARSE", e.to_string(), false),
        IngestError::Config(msg) => err("SCAN_CONFIG", msg, false),
        IngestError::NoHome => err(
            "SCAN_NO_HOME",
            "home directory could not be determined — set $HOME",
            false,
        ),
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::ProjectId;
    use vestige_engine::{DiscoveredSession, NormalizedTurn};

    // === fake source ===

    /// In-memory [`SessionSource`] so the batch logic can be tested without the
    /// filesystem / dash-encoding gymnastics of the real Claude Code adapter.
    struct FakeSource {
        name: &'static str,
        sessions: Vec<(DiscoveredSession, Vec<NormalizedTurn>)>,
    }

    impl SessionSource for FakeSource {
        fn source_name(&self) -> &'static str {
            self.name
        }
        fn discover(&self) -> Result<Vec<DiscoveredSession>, IngestError> {
            Ok(self.sessions.iter().map(|(s, _)| s.clone()).collect())
        }
        fn read_turns(
            &self,
            session: &DiscoveredSession,
            from_line: usize,
        ) -> Result<Vec<NormalizedTurn>, IngestError> {
            let turns = self
                .sessions
                .iter()
                .find(|(s, _)| s.session_id == session.session_id)
                .map(|(_, t)| t)
                .expect("fake session present");
            // Mirror the real adapter: `from_line` is a 0-based offset; emit turns
            // whose 1-based line number is strictly greater.
            Ok(turns
                .iter()
                .filter(|t| t.line > from_line)
                .cloned()
                .collect())
        }
    }

    fn turn(role: &str, text: &str, line: usize) -> NormalizedTurn {
        NormalizedTurn {
            role: role.to_string(),
            text: text.to_string(),
            line,
        }
    }

    fn session(id: &str, path: &str, project: &ProjectId) -> DiscoveredSession {
        DiscoveredSession {
            session_id: id.to_string(),
            file_path: PathBuf::from(path),
            project_id: project.clone(),
        }
    }

    fn open_store(slug: &str) -> (TempDir, Store, ProjectId) {
        let tmp = TempDir::new().unwrap();
        let project_id = ProjectId::from_slug(slug);
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        store
            .ensure_project(&project_id, "scan test", None, None)
            .unwrap();
        (tmp, store, project_id)
    }

    // === tests ===

    #[test]
    fn default_max_turns_is_100() {
        assert_eq!(default_max_turns(), 100);
    }

    #[test]
    fn redacts_and_advances_cursor_idempotently() {
        let (_tmp, store, project) = open_store("scan-redact");
        let sources: Vec<Box<dyn SessionSource>> = vec![Box::new(FakeSource {
            name: "claude_code",
            sessions: vec![(
                session("s1", "/fake/s1.jsonl", &project),
                vec![
                    turn("user", "use bearer sk-secret123456 in the header", 1),
                    turn("assistant", "noted", 2),
                ],
            )],
        })];

        let first = collect_batch(&sources, &store, &project, 100).unwrap();
        assert_eq!(first.turns.len(), 2);
        assert_eq!(first.sessions_scanned, 1);
        assert!(first.cursor_advanced);
        // Secret scrubbed by redact_secrets.
        assert!(!first.turns[0].text.contains("sk-secret123456"));
        assert_eq!(first.turns[0].source_ref, "claude_code:s1:L1");

        // Idempotent re-call: cursor already at line 2 → nothing new.
        let second = collect_batch(&sources, &store, &project, 100).unwrap();
        assert!(second.turns.is_empty());
        assert!(!second.cursor_advanced);
    }

    #[test]
    fn project_scope_excludes_other_projects() {
        let (_tmp, store, project_a) = open_store("scan-proj-a");
        let project_b = ProjectId::from_slug("scan-proj-b");
        let sources: Vec<Box<dyn SessionSource>> = vec![Box::new(FakeSource {
            name: "claude_code",
            sessions: vec![(
                session("sb", "/fake/sb.jsonl", &project_b),
                vec![turn("user", "belongs to B", 1)],
            )],
        })];

        // Serving project A: B's session must never surface.
        let out = collect_batch(&sources, &store, &project_a, 100).unwrap();
        assert!(out.turns.is_empty());
        assert_eq!(out.sessions_scanned, 0);
    }

    #[test]
    fn max_turns_budget_caps_and_resumes() {
        let (_tmp, store, project) = open_store("scan-budget");
        let sources: Vec<Box<dyn SessionSource>> = vec![Box::new(FakeSource {
            name: "claude_code",
            sessions: vec![(
                session("s1", "/fake/s1.jsonl", &project),
                vec![
                    turn("user", "one", 1),
                    turn("assistant", "two", 2),
                    turn("user", "three", 3),
                    turn("assistant", "four", 4),
                ],
            )],
        })];

        let first = collect_batch(&sources, &store, &project, 2).unwrap();
        assert_eq!(first.turns.len(), 2);
        assert_eq!(first.turns[1].line, 2);
        assert!(first.cursor_advanced);

        // Remaining turns resurface — budget dropped them, cursor only moved to line 2.
        let second = collect_batch(&sources, &store, &project, 2).unwrap();
        assert_eq!(second.turns.len(), 2);
        assert_eq!(second.turns[0].text, "three");
        assert_eq!(second.turns[1].text, "four");

        let third = collect_batch(&sources, &store, &project, 2).unwrap();
        assert!(third.turns.is_empty());
    }
}
