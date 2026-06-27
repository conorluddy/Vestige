//! One-shot session-log scan → candidate proposal (V0.5.4, daemon + CLI).
//!
//! [`scan_and_propose`] is the single source of truth for **autonomous** session-log
//! ingestion: it reads new transcript turns past each per-file watermark, redacts secrets,
//! hands a batch to an [`ExtractionProvider`], and routes whatever it proposes through the
//! existing [`propose_candidate`](crate::propose_candidate) path (the V0.2 inbox) — candidates
//! are **never** auto-promoted. Both the daemon's `session_log_scan` job and the one-shot
//! `vestige scan` CLI call this function; they differ only in how they build the provider.
//!
//! This is the autonomous analogue of the agent-driven `vestige_scan_sessions` MCP tool:
//! the MCP tool hands redacted turns to the *calling agent*; here a configured
//! [`ExtractionProvider`] does the extraction unattended. The cursor, redaction, and
//! project-scope invariants are identical.

use std::path::PathBuf;

use tracing::{debug, warn};

use vestige_core::{redact_secrets, NewCandidate, NewCandidateSource, NormalizedTurn, ProjectId};
use vestige_extract::ExtractionProvider;
use vestige_store::Store;

use super::source::SessionSource;
use super::{ClaudeCodeSource, CodexSource, IngestError};
use crate::candidate::propose_candidate;
use crate::error::Result;

/// Source-kind string stamped on every candidate proposed by a scan (matches
/// `vestige_core::SourceKind::SessionLog`).
const SESSION_LOG_SOURCE_KIND: &str = "session_log";

/// Default cap on turns fed to the extractor per session per scan. Bounds token usage; the
/// remainder of a very long session resurfaces on the next scan via the watermark.
const DEFAULT_MAX_TURNS_PER_SESSION: usize = 200;

// === PUBLIC TYPES ===

/// Options controlling a [`scan_and_propose`] run.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// When `true`, extract and preview but write nothing: no candidates proposed, no cursor
    /// advanced. Powers `vestige scan --dry-run`.
    pub dry_run: bool,
    /// Maximum turns handed to the extractor per session per scan.
    pub max_turns_per_session: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            max_turns_per_session: DEFAULT_MAX_TURNS_PER_SESSION,
        }
    }
}

/// A single proposed (or, in dry-run, would-be) candidate, for human/agent display.
#[derive(Debug, Clone)]
pub struct ScanPreview {
    /// Memory type the extractor assigned (`"decision"`, `"note"`, …).
    pub proposed_type: String,
    /// The candidate body.
    pub body: String,
    /// Provenance ref: `"<source>:<session_id>"`.
    pub source_ref: String,
}

/// Aggregate outcome of one [`scan_and_propose`] run over a project.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    /// In-scope sessions inspected this run.
    pub sessions_scanned: usize,
    /// Turns read past the watermark and considered this run.
    pub turns_processed: usize,
    /// Candidates actually proposed (always `0` in dry-run).
    pub candidates_proposed: usize,
    /// `true` when at least one per-file watermark moved forward.
    pub cursor_advanced: bool,
    /// Echoes [`ScanOptions::dry_run`].
    pub dry_run: bool,
    /// One entry per proposed (or would-be) candidate.
    pub previews: Vec<ScanPreview>,
}

// === PUBLIC API ===

/// Build the default session sources: Claude Code and Codex transcripts.
///
/// Honours `VESTIGE_CLAUDE_ROOT` / `VESTIGE_CODEX_ROOT` as test seams so a harness can point
/// each adapter at a tempdir. Shared by the daemon job and the `vestige scan` CLI so source
/// discovery stays identical to the agent-driven MCP path.
pub fn build_sources() -> std::result::Result<Vec<Box<dyn SessionSource>>, IngestError> {
    let claude = match std::env::var_os("VESTIGE_CLAUDE_ROOT") {
        Some(root) => ClaudeCodeSource::with_root(PathBuf::from(root)),
        None => ClaudeCodeSource::new()?,
    };
    let codex = match std::env::var_os("VESTIGE_CODEX_ROOT") {
        Some(root) => CodexSource::with_root(PathBuf::from(root)),
        None => CodexSource::new()?,
    };
    Ok(vec![Box::new(claude), Box::new(codex)])
}

/// Scan `sources` for new turns in `project_id`, extract candidates via `extractor`, and
/// propose each through the V0.2 inbox.
///
/// Per session (scoped to `project_id`):
/// 1. Read the watermark, then read transcript turns past it.
/// 2. Redact secrets in every turn (secrets never reach the extractor).
/// 3. Hand up to [`ScanOptions::max_turns_per_session`] non-empty turns to `extractor`.
/// 4. Map each [`vestige_extract::ExtractedCandidate`] to a [`NewCandidate`] with
///    `session_log` provenance and call [`propose_candidate`].
/// 5. Advance the watermark to the last turn read (skipped entirely in dry-run).
///
/// **Robustness:** a per-session extraction error is logged at `warn` and that session is
/// skipped *without* advancing its cursor (so it is retried next scan) — the extractor never
/// dumps raw turns as candidates. Discovery / transcript-read I/O errors propagate as
/// [`EngineError::Ingest`](crate::error::EngineError::Ingest).
pub fn scan_and_propose(
    sources: &[Box<dyn SessionSource>],
    store: &mut Store,
    project_id: &ProjectId,
    extractor: &dyn ExtractionProvider,
    opts: &ScanOptions,
) -> Result<ScanReport> {
    let mut report = ScanReport {
        dry_run: opts.dry_run,
        ..Default::default()
    };

    for source in sources {
        let source_name = source.source_name();
        let discovered = source.discover()?;

        for session in discovered {
            // Project-scope boundary: never touch another project's sessions.
            if &session.project_id != project_id {
                continue;
            }
            report.sessions_scanned += 1;

            let file_path_str = session.file_path.to_string_lossy().to_string();
            let from = store
                .get_scan_cursor(source_name, &file_path_str)?
                .map(|c| c.last_offset.max(0) as usize)
                .unwrap_or(0);

            let read = source.read_turns(&session, from)?;

            // Take at most `max_turns_per_session`; redact each, tracking the highest line
            // actually consumed so the cursor only advances over what we processed.
            let mut last_line: Option<usize> = None;
            let mut batch: Vec<NormalizedTurn> = Vec::new();
            for turn in read.into_iter().take(opts.max_turns_per_session) {
                report.turns_processed += 1;
                last_line = Some(turn.line);
                let text = redact_secrets(&turn.text);
                if text.trim().is_empty() {
                    continue;
                }
                batch.push(NormalizedTurn {
                    role: turn.role,
                    text,
                    line: turn.line,
                });
            }

            if !batch.is_empty() {
                match extractor.extract(&batch) {
                    Ok(extracted) => {
                        for cand in extracted {
                            let source_ref = format!("{source_name}:{}", session.session_id);
                            report.previews.push(ScanPreview {
                                proposed_type: cand.proposed_type.as_str().to_string(),
                                body: cand.body.clone(),
                                source_ref: source_ref.clone(),
                            });
                            if !opts.dry_run {
                                let confidence = cand.confidence.clamp(0.0, 1.0);
                                let new_candidate = NewCandidate {
                                    project_id: project_id.clone(),
                                    proposed_type: cand.proposed_type,
                                    body: cand.body,
                                    rationale: cand.rationale,
                                    title_override: None,
                                    importance: confidence,
                                    confidence,
                                    source: Some(NewCandidateSource {
                                        source_type: SESSION_LOG_SOURCE_KIND.to_string(),
                                        source_ref: Some(source_ref),
                                        source_content: None,
                                    }),
                                    duplicate_of_memory_id: None,
                                    duplicate_of_candidate_id: None,
                                };
                                propose_candidate(store, project_id, new_candidate)?;
                                report.candidates_proposed += 1;
                            }
                        }
                    }
                    Err(e) => {
                        // No-op + warn: never dump raw turns, and leave the cursor put so the
                        // session is retried on the next scan.
                        warn!(
                            project = %project_id.as_str(),
                            source = source_name,
                            session = %session.session_id,
                            error = %e,
                            "session-log extraction failed; skipping session without advancing cursor"
                        );
                        continue;
                    }
                }
            } else {
                debug!(
                    source = source_name,
                    session = %session.session_id,
                    "no non-empty turns to extract"
                );
            }

            // Advance the watermark over everything we read (skipped in dry-run).
            if !opts.dry_run {
                if let Some(line) = last_line {
                    if line > from {
                        store.record_scan_cursor(
                            source_name,
                            &file_path_str,
                            &session.project_id,
                            line as i64,
                        )?;
                        report.cursor_advanced = true;
                    }
                }
            }
        }
    }

    Ok(report)
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vestige_core::MemoryType;
    use vestige_extract::{ExtractError, ExtractedCandidate, FakeExtractionProvider};

    use crate::ingest::DiscoveredSession;

    /// In-memory [`SessionSource`] so the scan logic can be tested without the filesystem.
    struct FakeSource {
        name: &'static str,
        sessions: Vec<(DiscoveredSession, Vec<NormalizedTurn>)>,
    }

    impl SessionSource for FakeSource {
        fn source_name(&self) -> &'static str {
            self.name
        }
        fn discover(&self) -> std::result::Result<Vec<DiscoveredSession>, IngestError> {
            Ok(self.sessions.iter().map(|(s, _)| s.clone()).collect())
        }
        fn read_turns(
            &self,
            session: &DiscoveredSession,
            from_line: usize,
        ) -> std::result::Result<Vec<NormalizedTurn>, IngestError> {
            let turns = self
                .sessions
                .iter()
                .find(|(s, _)| s.session_id == session.session_id)
                .map(|(_, t)| t)
                .expect("fake session present");
            Ok(turns
                .iter()
                .filter(|t| t.line > from_line)
                .cloned()
                .collect())
        }
    }

    /// Extractor that always proposes exactly one decision per non-empty batch.
    struct OneDecisionExtractor;
    impl ExtractionProvider for OneDecisionExtractor {
        fn provider_name(&self) -> &'static str {
            "test"
        }
        fn model_name(&self) -> &str {
            "test"
        }
        fn extract(
            &self,
            turns: &[NormalizedTurn],
        ) -> std::result::Result<Vec<ExtractedCandidate>, ExtractError> {
            if turns.is_empty() {
                return Err(ExtractError::EmptyInput);
            }
            Ok(vec![ExtractedCandidate {
                proposed_type: MemoryType::Decision,
                body: format!("decision from {} turns", turns.len()),
                rationale: Some("test".into()),
                confidence: 0.8,
            }])
        }
    }

    /// Extractor that always errors (model unreachable) — must not advance the cursor.
    struct FailingExtractor;
    impl ExtractionProvider for FailingExtractor {
        fn provider_name(&self) -> &'static str {
            "failing"
        }
        fn model_name(&self) -> &str {
            "failing"
        }
        fn extract(
            &self,
            _turns: &[NormalizedTurn],
        ) -> std::result::Result<Vec<ExtractedCandidate>, ExtractError> {
            Err(ExtractError::Network("unreachable".into()))
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

    fn sources_with(
        name: &'static str,
        sess: DiscoveredSession,
        turns: Vec<NormalizedTurn>,
    ) -> Vec<Box<dyn SessionSource>> {
        vec![Box::new(FakeSource {
            name,
            sessions: vec![(sess, turns)],
        })]
    }

    #[test]
    fn proposes_candidates_and_advances_cursor_idempotently() {
        let (_tmp, mut store, project) = open_store("scan-engine-basic");
        let sources = sources_with(
            "claude_code",
            session("s1", "/fake/s1.jsonl", &project),
            vec![
                turn("user", "we will use SQLite as the store", 1),
                turn("assistant", "noted", 2),
            ],
        );

        let extractor = OneDecisionExtractor;
        let report = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &extractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(report.sessions_scanned, 1);
        assert_eq!(report.candidates_proposed, 1);
        assert!(report.cursor_advanced);
        assert_eq!(store.pending_candidate_count(&project).unwrap(), 1);

        // Idempotent re-scan: cursor at line 2 → nothing new.
        let again = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &extractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(again.candidates_proposed, 0);
        assert!(!again.cursor_advanced);
        assert_eq!(store.pending_candidate_count(&project).unwrap(), 1);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (_tmp, mut store, project) = open_store("scan-engine-dry");
        let sources = sources_with(
            "claude_code",
            session("s1", "/fake/s1.jsonl", &project),
            vec![turn("user", "decide to ship V0.5.4", 1)],
        );

        let opts = ScanOptions {
            dry_run: true,
            ..Default::default()
        };
        let report =
            scan_and_propose(&sources, &mut store, &project, &OneDecisionExtractor, &opts).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.candidates_proposed, 0);
        assert_eq!(report.previews.len(), 1);
        assert!(!report.cursor_advanced);
        assert_eq!(store.pending_candidate_count(&project).unwrap(), 0);

        // Cursor untouched, so a real scan afterwards still sees the turn.
        let real = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &OneDecisionExtractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(real.candidates_proposed, 1);
    }

    #[test]
    fn project_scope_excludes_other_projects() {
        let (_tmp, mut store, project_a) = open_store("scan-engine-a");
        let project_b = ProjectId::from_slug("scan-engine-b");
        let sources = sources_with(
            "claude_code",
            session("sb", "/fake/sb.jsonl", &project_b),
            vec![turn("user", "belongs to B", 1)],
        );

        let report = scan_and_propose(
            &sources,
            &mut store,
            &project_a,
            &OneDecisionExtractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(report.sessions_scanned, 0);
        assert_eq!(report.candidates_proposed, 0);
    }

    #[test]
    fn redacts_secrets_before_extraction() {
        let (_tmp, mut store, project) = open_store("scan-engine-redact");
        let sources = sources_with(
            "claude_code",
            session("s1", "/fake/s1.jsonl", &project),
            vec![turn("user", "key is sk-supersecret1234567890", 1)],
        );

        // Capture what the extractor receives by proposing the batch text as the body.
        struct EchoExtractor;
        impl ExtractionProvider for EchoExtractor {
            fn provider_name(&self) -> &'static str {
                "echo"
            }
            fn model_name(&self) -> &str {
                "echo"
            }
            fn extract(
                &self,
                turns: &[NormalizedTurn],
            ) -> std::result::Result<Vec<ExtractedCandidate>, ExtractError> {
                Ok(vec![ExtractedCandidate {
                    proposed_type: MemoryType::Note,
                    body: turns
                        .iter()
                        .map(|t| t.text.clone())
                        .collect::<Vec<_>>()
                        .join(" "),
                    rationale: None,
                    confidence: 0.5,
                }])
            }
        }

        let report = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &EchoExtractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(report.previews.len(), 1);
        assert!(
            !report.previews[0].body.contains("sk-supersecret1234567890"),
            "secret must be redacted before reaching the extractor"
        );
    }

    #[test]
    fn extraction_error_skips_session_without_advancing_cursor() {
        let (_tmp, mut store, project) = open_store("scan-engine-fail");
        let sources = sources_with(
            "claude_code",
            session("s1", "/fake/s1.jsonl", &project),
            vec![turn("user", "something worth keeping", 1)],
        );

        let report = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &FailingExtractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(report.candidates_proposed, 0);
        assert!(
            !report.cursor_advanced,
            "cursor must not advance on extraction failure"
        );

        // A subsequent scan with a working extractor still sees the turn.
        let recovered = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &OneDecisionExtractor,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(recovered.candidates_proposed, 1);
    }

    #[test]
    fn fake_extraction_provider_integration() {
        // Sanity: the always-compiled FakeExtractionProvider drives the pipeline.
        let (_tmp, mut store, project) = open_store("scan-engine-fakeprov");
        let sources = sources_with(
            "claude_code",
            session("s1", "/fake/s1.jsonl", &project),
            vec![turn("user", "a sufficiently long sentence to keep", 1)],
        );
        let provider = FakeExtractionProvider::default();
        let report = scan_and_propose(
            &sources,
            &mut store,
            &project,
            &provider,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(report.candidates_proposed, 1);
    }
}
