//! [`ClaudeCodeSource`] — session-log adapter for Claude Code (`~/.claude/projects`).
//!
//! Claude Code writes one `.jsonl` file per session under a directory whose name is the
//! current working directory with `/` replaced by `-` (leading `/` becomes a leading `-`):
//!
//! ```text
//! ~/.claude/projects/
//!   -Users-conor-Development-Vestige/
//!     <uuid>.jsonl
//!     <uuid>.jsonl
//!   -Users-conor-Development-Extoken/
//!     <uuid>.jsonl
//! ```
//!
//! Each `.jsonl` line is a JSON object. We extract a `role` field and a `content` / `text`
//! field, tolerating lines that don't match the expected schema.
//!
//! # Project mapping
//!
//! For each discovered session directory the decoded cwd is passed to
//! [`vestige_config::paths::discover_config`], which walks UP from the cwd looking for a
//! `.vestige/config.toml`. This covers sessions in subdirectories of a repo. Sessions whose
//! cwd maps to no registered project are skipped (logged, never misattributed).
//!
//! **Future:** the daemon registry (`vestige-daemon/src/registry.rs`) holds a heavier
//! `~/.vestige/projects/*` SQLite scan that could complement this fast path. Extracting it
//! is a separate task; the `discover_config` walk is sufficient for Wave 1.

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use tracing::{debug, warn};
use vestige_core::ProjectId;

use super::{
    source::{DiscoveredSession, NormalizedTurn, SessionSource},
    IngestError,
};

// === PUBLIC TYPE ===

/// Adapter that discovers Claude Code transcripts under `~/.claude/projects`.
///
/// Construct with [`ClaudeCodeSource::new`] (defaults root to `~/.claude/projects`)
/// or [`ClaudeCodeSource::with_root`] for tests or alternate installations.
pub struct ClaudeCodeSource {
    root: PathBuf,
}

impl ClaudeCodeSource {
    /// Construct a source rooted at `~/.claude/projects`.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::NoHome`] if the home directory cannot be resolved.
    pub fn new() -> Result<Self, IngestError> {
        let home = home_dir().ok_or(IngestError::NoHome)?;
        Ok(Self {
            root: home.join(".claude").join("projects"),
        })
    }

    /// Construct a source rooted at an explicit path (for tests or alternate installations).
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SessionSource for ClaudeCodeSource {
    fn source_name(&self) -> &'static str {
        "claude_code"
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>, IngestError> {
        let mut sessions = Vec::new();

        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(root = %self.root.display(), "claude_code root not found, skipping discover");
                return Ok(vec![]);
            }
            Err(e) => return Err(IngestError::Io(e)),
        };

        for entry in entries {
            let entry = entry?;
            let dir_path = entry.path();

            if !dir_path.is_dir() {
                continue;
            }

            let dir_name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => {
                    debug!(path = %dir_path.display(), "skipping non-UTF-8 directory name");
                    continue;
                }
            };

            let cwd = decode_cwd(&dir_name);

            let project_id = match resolve_project(&cwd) {
                Some(id) => id,
                None => {
                    debug!(
                        cwd = %cwd.display(),
                        "no registered project for cwd, skipping session directory"
                    );
                    continue;
                }
            };

            let file_entries = match fs::read_dir(&dir_path) {
                Ok(e) => e,
                Err(e) => {
                    warn!(path = %dir_path.display(), error = %e, "could not read session directory");
                    continue;
                }
            };

            for file_entry in file_entries {
                let file_entry = match file_entry {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(error = %e, "error reading directory entry, skipping");
                        continue;
                    }
                };

                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

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
                    project_id: project_id.clone(),
                });
            }
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

            // Tolerant parse: skip lines that don't look like message objects.
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

/// Decode a Claude Code dash-encoded directory name back to a filesystem path.
///
/// Claude Code encodes the session's cwd by replacing every `/` with `-` (the
/// leading `/` becomes a leading `-`). Decoding reverses this: a leading `-`
/// is replaced with `/`, then every remaining `-` is replaced with `/`.
///
/// ```text
/// "-Users-conor-Development-Foo" → "/Users/conor/Development/Foo"
/// ```
///
/// **Limitation:** directory components that contain a literal `-` are
/// indistinguishable from path separators in this encoding. This is a known
/// limitation of the upstream Claude Code format, not a Vestige-specific issue.
fn decode_cwd(dir_name: &str) -> PathBuf {
    // Leading `-` represents the leading `/` in an absolute path.
    let path_str = if let Some(rest) = dir_name.strip_prefix('-') {
        format!("/{}", rest.replace('-', "/"))
    } else {
        dir_name.replace('-', "/")
    };
    PathBuf::from(path_str)
}

/// Try to resolve a cwd to a registered `ProjectId` via `vestige-config`.
///
/// Uses [`vestige_config::paths::discover_config`] which walks UP from `cwd`
/// looking for `.vestige/config.toml`. Returns `None` on any miss (config not
/// found, invalid project_id prefix, etc.).
///
/// **Note:** a heavier scan via the daemon registry
/// (`vestige-daemon/src/registry.rs`) is a future task. The `discover_config`
/// walk is sufficient for Wave 1.
fn resolve_project(cwd: &Path) -> Option<ProjectId> {
    let (_cfg_path, cfg) = vestige_config::paths::discover_config(cwd).ok()?;
    cfg.project_id().ok()
}

/// Extract a [`NormalizedTurn`] from a parsed JSON line, returning `None` if the
/// object does not contain a recognisable role + content pair.
///
/// Claude Code transcript format heuristic:
/// - `message.role` + `message.content[].text` (nested message object)
/// - `role` + `content` (flat object)
/// - `role` + `text` (alternate flat)
///
/// We apply these in order and return the first match. Any turn without a
/// usable role or text is silently skipped.
fn extract_turn(value: &serde_json::Value, line_number: usize) -> Option<NormalizedTurn> {
    // Path 1: nested `{ message: { role, content: [{ text }] } }` shape.
    if let Some(message) = value.get("message") {
        if let Some(turn) = extract_flat_turn(message, line_number) {
            return Some(turn);
        }
    }

    // Path 2: flat `{ role, content }` or `{ role, text }`.
    extract_flat_turn(value, line_number)
}

/// Extract role + text from a flat JSON object.
fn extract_flat_turn(obj: &serde_json::Value, line_number: usize) -> Option<NormalizedTurn> {
    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;

    // Try `content` first (may be a string or an array of content blocks).
    let text = if let Some(content) = obj.get("content") {
        match content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(blocks) => {
                // Concatenate text blocks, skipping non-text entries.
                let parts: Vec<&str> = blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect();
                if parts.is_empty() {
                    // Content array has no text blocks — fall through to `text` field.
                    obj.get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    parts.join("\n")
                }
            }
            _ => obj
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        }
    } else if let Some(text_val) = obj.get("text").and_then(|v| v.as_str()) {
        text_val.to_string()
    } else {
        // No usable text field — skip this turn.
        return None;
    };

    Some(NormalizedTurn {
        role,
        text,
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
    use vestige_core::ProjectId;

    // --- decode_cwd ---

    #[test]
    fn decode_cwd_absolute_path_round_trips() {
        let encoded = "-Users-conor-Development-Foo";
        let decoded = decode_cwd(encoded);
        assert_eq!(decoded, PathBuf::from("/Users/conor/Development/Foo"));
    }

    #[test]
    fn decode_cwd_single_component() {
        assert_eq!(decode_cwd("-tmp"), PathBuf::from("/tmp"));
    }

    #[test]
    fn decode_cwd_no_leading_dash_is_relative() {
        // A dir name without a leading `-` decodes as a relative path.
        assert_eq!(decode_cwd("foo-bar"), PathBuf::from("foo/bar"));
    }

    // --- read_turns ---

    fn make_project_root(tmp: &TempDir) -> (PathBuf, ProjectId) {
        let project_root = tmp.path().join("my-project");
        fs::create_dir_all(&project_root).unwrap();

        let project_id = ProjectId::from_slug("test-project");
        let config_path = project_root.join(".vestige").join("config.toml");
        let config = build_init_config(
            &project_id,
            "Test Project",
            &project_root
                .join(".vestige")
                .join("projects")
                .join("test-project")
                .join("memory.sqlite"),
        );
        write_config(&config_path, &config).unwrap();

        (project_root, project_id)
    }

    #[test]
    fn read_turns_parses_flat_role_content() {
        let tmp = TempDir::new().unwrap();
        let (project_root, project_id) = make_project_root(&tmp);

        let session_dir = tmp.path().join("sessions");
        fs::create_dir_all(&session_dir).unwrap();

        let jsonl = r#"{"role":"user","content":"Hello world"}
{"role":"assistant","content":"Hi there"}
"#;
        let jsonl_path = session_dir.join("test-session-uuid.jsonl");
        fs::write(&jsonl_path, jsonl).unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "test-session-uuid".to_string(),
            file_path: jsonl_path,
            project_id,
        };

        let turns = source.read_turns(&session, 0).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "Hello world");
        assert_eq!(turns[0].line, 1);
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "Hi there");
        assert_eq!(turns[1].line, 2);

        // Ensure project_root is in scope to keep its tempdir alive.
        let _ = project_root;
    }

    #[test]
    fn read_turns_respects_from_line_watermark() {
        let tmp = TempDir::new().unwrap();
        let (project_root, project_id) = make_project_root(&tmp);

        let jsonl = r#"{"role":"user","content":"first"}
{"role":"assistant","content":"second"}
{"role":"user","content":"third"}
"#;
        let jsonl_path = tmp.path().join("session.jsonl");
        fs::write(&jsonl_path, jsonl).unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "s".to_string(),
            file_path: jsonl_path,
            project_id,
        };

        let turns = source.read_turns(&session, 1).unwrap();
        // from_line = 1 means skip line index 0 (line 1), start at index 1 (line 2).
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "second");
        assert_eq!(turns[0].line, 2);
        assert_eq!(turns[1].text, "third");
        assert_eq!(turns[1].line, 3);

        let _ = project_root;
    }

    #[test]
    fn read_turns_tolerates_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let (_project_root, project_id) = make_project_root(&tmp);

        let jsonl = "not json at all\n{\"role\":\"user\",\"content\":\"good line\"}\n{bad}\n";
        let jsonl_path = tmp.path().join("session.jsonl");
        fs::write(&jsonl_path, jsonl).unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "s".to_string(),
            file_path: jsonl_path,
            project_id,
        };

        let turns = source.read_turns(&session, 0).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "good line");
        assert_eq!(turns[0].line, 2);
    }

    #[test]
    fn read_turns_parses_nested_message_shape() {
        let tmp = TempDir::new().unwrap();
        let (_project_root, project_id) = make_project_root(&tmp);

        // Claude Code nested format: { message: { role, content: [{ type: "text", text: "..." }] } }
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"nested turn"}]}}
"#;
        let jsonl_path = tmp.path().join("session.jsonl");
        fs::write(&jsonl_path, jsonl).unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let session = DiscoveredSession {
            session_id: "s".to_string(),
            file_path: jsonl_path,
            project_id,
        };

        let turns = source.read_turns(&session, 0).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "nested turn");
    }

    // --- discover / project mapping ---

    #[test]
    fn discover_skips_session_with_no_registered_project() {
        // Root has a dash-encoded dir that decodes to a path with no .vestige/config.toml.
        let tmp = TempDir::new().unwrap();

        // Encoded dir: -tmp-no-vestige-here (decodes to /tmp/no/vestige/here)
        // That path won't have a .vestige config, so the session should be skipped.
        let session_dir = tmp.path().join("-tmp-no-vestige-here");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("some-uuid.jsonl"), "{}\n").unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let sessions = source.discover().unwrap();
        assert!(
            sessions.is_empty(),
            "expected no sessions when cwd maps to no project; got {:?}",
            sessions.iter().map(|s| &s.session_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn discover_emits_session_for_registered_project() {
        let tmp = TempDir::new().unwrap();

        // Canonicalize before encoding: on macOS /tmp is a symlink to /private/tmp.
        // We also need the project root name to have NO hyphens — the Claude Code
        // dash-encoding is lossy (a hyphen in a directory name is indistinguishable
        // from a path separator), so both the TempDir basename and the project
        // sub-directory must be hyphen-free for the round-trip to work in tests.
        let canonical_base = tmp.path().canonicalize().unwrap();

        // Verify the canonical path is hyphen-free so the encode/decode round-trip
        // is unambiguous. If the OS-assigned temp dir name contains hyphens we skip
        // rather than produce a misleading failure.
        let canonical_str = canonical_base.to_str().unwrap();
        if canonical_str.contains('-') {
            // Not an error — the test just can't exercise this path on this machine.
            eprintln!(
                "skip: canonical TempDir path {canonical_str:?} contains hyphens \
                 (round-trip lossy for this OS temp dir name)"
            );
            return;
        }

        // "myproject" — no hyphens, so encode/decode round-trips cleanly.
        let project_root = canonical_base.join("myproject");
        fs::create_dir_all(&project_root).unwrap();
        let project_id = ProjectId::from_slug("myproject");
        let config = build_init_config(
            &project_id,
            "My Project",
            &project_root
                .join(".vestige")
                .join("projects")
                .join("myproject")
                .join("memory.sqlite"),
        );
        write_config(&project_root.join(".vestige").join("config.toml"), &config).unwrap();

        // Build a dash-encoded dir name for the project root.
        // e.g. /private/tmp/abc123/myproject → -private-tmp-abc123-myproject
        let encoded = encode_path_for_test(project_root.to_str().unwrap());
        let session_dir = tmp.path().join(&encoded);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("abc123.jsonl"), "{}\n").unwrap();

        let source = ClaudeCodeSource::with_root(tmp.path().to_path_buf());
        let sessions = source.discover().unwrap();
        assert_eq!(sessions.len(), 1, "expected one discovered session");
        assert_eq!(sessions[0].session_id, "abc123");
        assert_eq!(sessions[0].project_id.as_str(), project_id.as_str());
    }

    /// Encode a real absolute path to the Claude Code dash-encoding for test fixtures.
    fn encode_path_for_test(abs_path: &str) -> String {
        // leading `/` → `-`, then replace all remaining `/` with `-`
        if let Some(rest) = abs_path.strip_prefix('/') {
            format!("-{}", rest.replace('/', "-"))
        } else {
            abs_path.replace('/', "-")
        }
    }
}
