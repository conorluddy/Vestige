//! [`SessionSource`] trait and shared ingestion types.
//!
//! Mirrors the synchronous [`EmbeddingProvider`](vestige_embed::EmbeddingProvider) shape:
//! `Send + Sync` supertraits, plain sync methods, no `async`, no associated types.
//!
//! New transcript adapters implement this trait; the ingestion pipeline calls them through
//! `dyn SessionSource` so CLI / MCP / daemon can drive discovery without knowing the
//! concrete adapter.

use std::path::PathBuf;

use vestige_core::ProjectId;

use super::IngestError;

// === PUBLIC TYPES ===

/// A normalised conversational turn extracted from a coding-agent transcript.
///
/// Defined in `vestige-core` ([`vestige_core::NormalizedTurn`]) and re-exported here
/// so the source layer and the `vestige-extract` crate share one type without crossing
/// the one-way crate boundary. All adapters normalise their source format into this
/// common shape so downstream processing (candidate proposal, redaction, deduplication,
/// LLM extraction) is source-agnostic.
pub use vestige_core::NormalizedTurn;

/// A transcript file discovered on disk, already mapped to a registered project.
///
/// Sessions whose decoded cwd matches no registered project are **not** represented
/// here — they are skipped during [`SessionSource::discover`] and logged at
/// `tracing::debug!` level.
#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    /// Stable session identifier extracted from the filename stem (e.g. a UUID).
    pub session_id: String,
    /// Absolute path to the `.jsonl` transcript file.
    pub file_path: PathBuf,
    /// The registered project this session belongs to (resolved from the decoded cwd).
    pub project_id: ProjectId,
}

// === TRAIT ===

/// A pluggable source of coding-agent transcripts.
///
/// Implementations are `Send + Sync` — a single instance may be shared across
/// CLI, MCP, and daemon contexts.
///
/// # Implementing a new adapter
///
/// 1. Implement [`source_name`] — return a short, stable `&'static str` (e.g. `"codex"`).
/// 2. Implement [`discover`] — scan the source root, decode session metadata, map each
///    session to a project via the cwd → `ProjectId` fast path. Skip no-match sessions.
/// 3. Implement [`read_turns`] — parse the transcript file from `from_line` onward,
///    returning [`NormalizedTurn`]s with correct 1-based line numbers. Be tolerant: per-line
///    parse errors should be swallowed with `.ok()` rather than aborting the batch.
///
/// [`source_name`]: SessionSource::source_name
/// [`discover`]: SessionSource::discover
/// [`read_turns`]: SessionSource::read_turns
pub trait SessionSource: Send + Sync {
    /// Stable, lowercase source identifier (e.g. `"claude_code"`, `"codex"`).
    ///
    /// Stored as provenance metadata alongside ingested candidates; must not change
    /// across versions for a given adapter (it becomes part of the audit trail).
    fn source_name(&self) -> &'static str;

    /// Discover transcript files under this source's root, each mapped to a registered project.
    ///
    /// Sessions that map to no registered project are **skipped** (logged at `debug!` / `warn!`
    /// level) and never misattributed to the wrong project.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::Io`] if the root directory cannot be read.
    fn discover(&self) -> Result<Vec<DiscoveredSession>, IngestError>;

    /// Parse and normalise a discovered session's turns from `from_line` onward.
    ///
    /// `from_line` is a 0-based offset into the file — pass `0` to read from the start,
    /// or a stored watermark to resume from where a previous scan left off. The returned
    /// [`NormalizedTurn`]s carry 1-based `line` numbers (watermark + 1 for the first turn).
    ///
    /// Implementations should be tolerant: per-line JSON parse errors and turns that lack
    /// a recognisable role/text should be silently skipped rather than bubbling an error.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::Io`] if the file cannot be opened or read.
    fn read_turns(
        &self,
        session: &DiscoveredSession,
        from_line: usize,
    ) -> Result<Vec<NormalizedTurn>, IngestError>;
}
