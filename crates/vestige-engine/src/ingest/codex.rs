//! [`CodexSource`] — session-log adapter for OpenAI Codex (`~/.codex/sessions`).
//!
//! Unlike Claude Code (one directory per cwd), Codex writes **date-partitioned** rollout
//! files and records the working directory *inside* the file rather than in the path:
//!
//! ```text
//! ~/.codex/sessions/
//!   2026/06/13/
//!     rollout-2026-06-13T17-05-24-<uuid>.jsonl
//!     rollout-2026-06-13T16-53-52-<uuid>.jsonl
//! ```
//!
//! The first JSONL record is a `session_meta` carrying `payload.cwd`. This divergence —
//! cwd-from-metadata rather than cwd-from-path — is the reason the [`SessionSource`] trait
//! exists: the discovery shape differs, but the normalised output does not.
//!
//! # Turn extraction
//!
//! Codex stores conversational turns as `response_item` records of `payload.type == "message"`,
//! each with a `payload.role` and a `payload.content` array of `{ type, text }` blocks. We
//! emit `user` and `assistant` roles only — `developer` / `system` records are framework
//! instructions, not conversation. The parallel `event_msg` stream is intentionally ignored
//! to avoid double-counting the same turn.
//!
//! # Project mapping
//!
//! The decoded cwd is passed to [`vestige_config::paths::discover_config`], which walks UP
//! looking for a `.vestige/config.toml`. Sessions whose cwd maps to no registered project
//! are skipped (logged, never misattributed) — identical to the Claude Code adapter.

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use tracing::debug;
use vestige_core::ProjectId;

use super::{
    source::{DiscoveredSession, NormalizedTurn, SessionSource},
    IngestError,
};

// === PUBLIC TYPE ===

/// Adapter that discovers Codex rollout transcripts under `~/.codex/sessions`.
///
/// Construct with [`CodexSource::new`] (defaults root to `~/.codex/sessions`)
/// or [`CodexSource::with_root`] for tests or alternate installations.
pub struct CodexSource {
    root: PathBuf,
}

impl CodexSource {
    /// Construct a source rooted at `~/.codex/sessions`.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::NoHome`] if the home directory cannot be resolved.
    pub fn new() -> Result<Self, IngestError> {
        let home = home_dir().ok_or(IngestError::NoHome)?;
        Ok(Self {
            root: home.join(".codex").join("sessions"),
        })
    }

    /// Construct a source rooted at an explicit path (for tests or alternate installations).
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SessionSource for CodexSource {
    fn source_name(&self) -> &'static str {
        "codex"
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>, IngestError> {
        let mut files = Vec::new();
        collect_rollout_files(&self.root, &mut files)?;

        let mut sessions = Vec::new();
        for file_path in files {
            let cwd = match read_cwd_from_meta(&file_path) {
                Some(c) => c,
                None => {
                    debug!(path = %file_path.display(), "no session_meta cwd, skipping codex session");
                    continue;
                }
            };

            let project_id = match resolve_project(&cwd) {
                Some(id) => id,
                None => {
                    debug!(
                        cwd = %cwd.display(),
                        "no registered project for cwd, skipping codex session"
                    );
                    continue;
                }
            };

            let session_id = match file_path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => {
                    debug!(path = %file_path.display(), "skipping file with non-UTF-8 stem");
                    continue;
                }
            };

            sessions.push(DiscoveredSession {
                session_id,
                file_path,
                project_id,
            });
        }

        Ok(sessions)
    }

    fn read_turns(
        &self,
        session: &DiscoveredSession,
        from_line: usize,
    ) -> Result<Vec<NormalizedTurn>, IngestError> {
        let file = fs::File::open(&session.file_path)?;
        let reader = BufReader::new(file);
        let mut turns = Vec::new();

        for (zero_idx, line_result) in reader.lines().enumerate() {
            // 1-based line number within the file.
            let line_number = zero_idx + 1;

            if zero_idx < from_line {
                continue;
            }

            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    debug!(line = line_number, error = %e, "read error, skipping line");
                    continue;
                }
            };

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Tolerant parse: skip lines that don't look like records.
            let value: serde_json::Value = match serde_json::from_str(line).ok() {
                Some(v) => v,
                None => {
                    debug!(line = line_number, "non-JSON line, skipping");
                    continue;
                }
            };

            if let Some(turn) = extract_turn(&value, line_number) {
                turns.push(turn);
            }
        }

        Ok(turns)
    }
}

// === PRIVATE HELPERS ===

/// Recursively collect `rollout-*.jsonl` files under `root` (Codex date-partitions
/// sessions as `YYYY/MM/DD/`). A missing root is not an error — discovery returns empty.
fn collect_rollout_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), IngestError> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!(root = %root.display(), "codex root not found, skipping discover");
            return Ok(());
        }
        Err(e) => return Err(IngestError::Io(e)),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rollout_files(&path, out)?;
        } else if is_rollout_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// A Codex transcript is a `rollout-*.jsonl` file.
fn is_rollout_file(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return false;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("rollout-"))
        .unwrap_or(false)
}

/// Read the `cwd` from the first `session_meta` record near the top of the file.
///
/// Scans the first few lines (the meta record is line 1 in practice, but we tolerate
/// leading blanks / reordering). Returns `None` if no `session_meta.payload.cwd` is found.
fn read_cwd_from_meta(file_path: &Path) -> Option<PathBuf> {
    let file = fs::File::open(file_path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(16) {
        // Tolerate a bad line (I/O / non-UTF-8) — keep scanning for the meta record
        // rather than dropping the whole session, matching `read_turns`.
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
            if let Some(cwd) = value
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .and_then(|c| c.as_str())
            {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

/// Try to resolve a cwd to a registered `ProjectId` via `vestige-config`.
///
/// Mirrors the Claude Code adapter: [`vestige_config::paths::discover_config`] walks UP
/// from `cwd` looking for `.vestige/config.toml`. Returns `None` on any miss.
fn resolve_project(cwd: &Path) -> Option<ProjectId> {
    let (_cfg_path, cfg) = vestige_config::paths::discover_config(cwd).ok()?;
    cfg.project_id().ok()
}

/// Extract a [`NormalizedTurn`] from a Codex record, returning `None` for non-message
/// records and for `developer` / `system` instruction messages.
///
/// Codex shape: `{ type: "response_item", payload: { type: "message", role, content: [{ text }] } }`.
fn extract_turn(value: &serde_json::Value, line_number: usize) -> Option<NormalizedTurn> {
    if value.get("type").and_then(|t| t.as_str()) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
        return None;
    }

    let role = payload.get("role").and_then(|r| r.as_str())?;
    // Only conversational turns — developer/system records are framework instructions.
    if role != "user" && role != "assistant" {
        return None;
    }

    let content = payload.get("content")?.as_array()?;
    let parts: Vec<&str> = content
        .iter()
        .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
        .collect();
    if parts.is_empty() {
        return None;
    }

    Some(NormalizedTurn {
        role: role.to_string(),
        text: parts.join("\n"),
        line: line_number,
    })
}

/// Resolve the home directory via `$HOME` then `directories::BaseDirs`.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use vestige_config::{build_init_config, paths::write_config};

    /// Write a `.vestige/config.toml` under `project_root` and return its `ProjectId`.
    fn make_project_root(project_root: &Path) -> ProjectId {
        fs::create_dir_all(project_root).unwrap();
        let project_id = ProjectId::from_slug("codex-test");
        let config = build_init_config(
            &project_id,
            "Codex Test",
            &project_root
                .join(".vestige")
                .join("projects")
                .join("codex-test")
                .join("memory.sqlite"),
        );
        write_config(&project_root.join(".vestige").join("config.toml"), &config).unwrap();
        project_id
    }

    const META_AND_TURNS: &str = r#"{"timestamp":"2026-06-13T16:05:24.497Z","type":"session_meta","payload":{"id":"abc","cwd":"__CWD__"}}
{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"you are codex, instructions"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"decide: use SQLite for the store"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"agreed, SQLite it is"}]}}
{"type":"event_msg","payload":{"type":"token_count"}}
"#;

    #[test]
    fn read_turns_extracts_user_and_assistant_only() {
        let tmp = TempDir::new().unwrap();
        let jsonl_path = tmp.path().join("rollout-2026-06-13T16-05-24-abc.jsonl");
        fs::write(&jsonl_path, META_AND_TURNS.replace("__CWD__", "/tmp/proj")).unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "rollout-abc".to_string(),
            file_path: jsonl_path,
            project_id: ProjectId::from_slug("codex-test"),
        };

        let turns = source.read_turns(&session, 0).unwrap();
        // developer message + token_count event are skipped; 2 conversational turns remain.
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "decide: use SQLite for the store");
        assert_eq!(turns[0].line, 3);
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "agreed, SQLite it is");
        assert_eq!(turns[1].line, 4);
    }

    #[test]
    fn read_turns_respects_from_line_watermark() {
        let tmp = TempDir::new().unwrap();
        let jsonl_path = tmp.path().join("rollout-x.jsonl");
        fs::write(&jsonl_path, META_AND_TURNS.replace("__CWD__", "/tmp/proj")).unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "x".to_string(),
            file_path: jsonl_path,
            project_id: ProjectId::from_slug("codex-test"),
        };

        // Resume past line 3 (the user turn) → only the assistant turn on line 4 remains.
        let turns = source.read_turns(&session, 3).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].role, "assistant");
        assert_eq!(turns[0].line, 4);
    }

    #[test]
    fn read_turns_tolerates_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let jsonl_path = tmp.path().join("rollout-y.jsonl");
        let jsonl = "not json\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"good\"}]}}\n{bad}\n";
        fs::write(&jsonl_path, jsonl).unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "y".to_string(),
            file_path: jsonl_path,
            project_id: ProjectId::from_slug("codex-test"),
        };

        let turns = source.read_turns(&session, 0).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "good");
        assert_eq!(turns[0].line, 2);
    }

    #[test]
    fn discover_reads_cwd_from_meta_and_maps_project() {
        let tmp = TempDir::new().unwrap();

        // Hyphen-free, canonicalised project root so config discovery is unambiguous.
        let canonical = tmp.path().canonicalize().unwrap();
        let project_root = canonical.join("myproject");
        let project_id = make_project_root(&project_root);

        // Date-partitioned session dir, cwd embedded in the meta record.
        let session_dir = tmp.path().join("2026").join("06").join("13");
        fs::create_dir_all(&session_dir).unwrap();
        let jsonl_path = session_dir.join("rollout-2026-06-13T17-05-24-uuid.jsonl");
        fs::write(
            &jsonl_path,
            META_AND_TURNS.replace("__CWD__", project_root.to_str().unwrap()),
        )
        .unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        let sessions = source.discover().unwrap();
        assert_eq!(sessions.len(), 1, "expected one discovered codex session");
        assert_eq!(sessions[0].project_id.as_str(), project_id.as_str());
        assert_eq!(sessions[0].session_id, "rollout-2026-06-13T17-05-24-uuid");
    }

    #[test]
    fn discover_skips_session_with_no_registered_project() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("2026").join("06").join("13");
        fs::create_dir_all(&session_dir).unwrap();
        // cwd points somewhere with no .vestige/config.toml.
        fs::write(
            session_dir.join("rollout-none.jsonl"),
            META_AND_TURNS.replace("__CWD__", "/tmp/definitely/not/a/vestige/project"),
        )
        .unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        let sessions = source.discover().unwrap();
        assert!(
            sessions.is_empty(),
            "expected no sessions when cwd maps to no project"
        );
    }

    #[test]
    fn discover_skips_session_without_meta_cwd() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("2026").join("06").join("13");
        fs::create_dir_all(&session_dir).unwrap();
        // No session_meta record → no cwd → skipped.
        fs::write(
            session_dir.join("rollout-nometa.jsonl"),
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"hi\"}]}}\n",
        )
        .unwrap();

        let source = CodexSource::with_root(tmp.path().to_path_buf());
        assert!(source.discover().unwrap().is_empty());
    }
}
