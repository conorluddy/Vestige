//! Candidate-memory types for the V0.2 assimilation inbox.
//!
//! A candidate is an agent-observed fact that has not yet been reviewed and
//! promoted to a full memory. The lifecycle is:
//!
//! ```text
//! Pending → Approved  (promoted to a Memory row in the store)
//!         → Rejected  (dismissed with a RejectionReason)
//!         → Superseded (replaced by a newer candidate)
//! ```
//!
//! All types here are pure domain — no `rusqlite`, no `clap`, no `rmcp`.
//! Persistence lives in `vestige-store`; this file owns the shapes.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{CoreError, Result};
use crate::ids::{CandidateId, MemoryId, ProjectId};
use crate::memory::{truncate_at_utf8_boundary, SOURCE_SNIPPET_MAX_BYTES};
use crate::representations::derive;
use crate::types::{CandidateStatus, MemoryType};

// === PUBLIC TYPES ===

/// Why a candidate was rejected (PRD §9.6).
///
/// Serialises as a lowercase snake_case string. Unrecognised strings
/// round-trip as [`RejectionReason::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Already captured as an existing memory or candidate.
    Duplicate,
    /// Factually incorrect or misleading.
    Wrong,
    /// Too ephemeral to be worth keeping.
    NotDurable,
    /// Too low signal-to-noise for the memory store.
    TooNoisy,
    /// Outdated — superseded by subsequent events.
    Stale,
    /// Any other reason; carries a free-form explanation.
    Other(String),
}

impl RejectionReason {
    /// Return the canonical string form. `Other` returns its inner string.
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Duplicate => Cow::Borrowed("duplicate"),
            Self::Wrong => Cow::Borrowed("wrong"),
            Self::NotDurable => Cow::Borrowed("not_durable"),
            Self::TooNoisy => Cow::Borrowed("too_noisy"),
            Self::Stale => Cow::Borrowed("stale"),
            Self::Other(s) => Cow::Borrowed(s.as_str()),
        }
    }
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl FromStr for RejectionReason {
    type Err = CoreError;
    /// Parse a rejection reason string. Empty input is an error; unrecognised
    /// non-empty strings become `Other(s)` so they round-trip without loss.
    fn from_str(s: &str) -> Result<Self> {
        if s.is_empty() {
            return Err(CoreError::InvalidRejectionReason {
                value: s.to_string(),
            });
        }
        match s.to_ascii_lowercase().as_str() {
            "duplicate" => Ok(Self::Duplicate),
            "wrong" => Ok(Self::Wrong),
            "not_durable" => Ok(Self::NotDurable),
            "too_noisy" => Ok(Self::TooNoisy),
            "stale" => Ok(Self::Stale),
            _ => Ok(Self::Other(s.to_string())),
        }
    }
}

impl Serialize for RejectionReason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for RejectionReason {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        RejectionReason::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

/// Source provenance attached to a candidate at capture time.
///
/// Mirrors [`SourceRow`](crate::SourceRow) for the candidate table.
/// Content is capped at [`SOURCE_SNIPPET_MAX_BYTES`] (2 KiB) before persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSource {
    /// Category of the source — e.g. `"file"`, `"url"`, `"clipboard"`.
    pub source_type: String,
    /// Stable locator (file path, URL, etc.) — `None` if not applicable.
    pub source_ref: Option<String>,
    /// Stored snippet, capped at 2 KiB. `None` if no content was provided.
    pub source_content: Option<String>,
    /// `true` when `source_content` was truncated to fit [`SOURCE_SNIPPET_MAX_BYTES`].
    pub truncated: bool,
}

/// Full candidate row as returned from the store.
///
/// Sources are populated when fetched with `include_sources`; otherwise
/// `sources` is empty. All fields mirror the PRD §8.3 schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Unique identifier — `cand_<ULID>`.
    pub id: CandidateId,
    /// Owning project — enforces the per-project scope boundary.
    pub project_id: ProjectId,
    /// Semantic classification of the proposed memory.
    pub proposed_type: MemoryType,
    /// Review lifecycle state.
    pub status: CandidateStatus,
    /// Short display label (≤ 60 chars), derived or overridden at capture.
    pub title: String,
    /// First sentence of the body — used in list views.
    pub one_liner: String,
    /// Full trimmed body text.
    pub summary: Option<String>,
    /// Verbatim full body without modification.
    pub full_body: String,
    /// Why the agent believes this is worth recording.
    pub rationale: Option<String>,
    /// Agent confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Author-supplied signal strength in `[0.0, 1.0]`.
    pub importance: f32,
    /// Set when this candidate duplicates an existing promoted memory.
    pub duplicate_of_memory_id: Option<MemoryId>,
    /// Set when this candidate duplicates another pending candidate.
    pub duplicate_of_candidate_id: Option<CandidateId>,
    /// Set when `status == Approved`; the memory row this was promoted into.
    pub approved_memory_id: Option<MemoryId>,
    /// Set when `status == Rejected`.
    pub rejection_reason: Option<RejectionReason>,
    /// Free-form reviewer note (approval or rejection).
    pub review_note: Option<String>,
    /// When the candidate row was first created (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When the candidate row was last mutated (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// When a reviewer acted on this candidate; `None` while still `Pending`.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<OffsetDateTime>,
    /// Attached sources — populated on demand; empty when not requested.
    pub sources: Vec<CandidateSource>,
}

/// Input shape for a single source attached to a new candidate.
#[derive(Debug, Clone)]
pub struct NewCandidateSource {
    /// Category of the source — e.g. `"file"`, `"url"`, `"clipboard"`.
    pub source_type: String,
    /// Stable locator, if provided.
    pub source_ref: Option<String>,
    /// Verbatim snippet to attach. Truncated to [`SOURCE_SNIPPET_MAX_BYTES`]
    /// (2 KiB) at a UTF-8 codepoint boundary before persistence.
    pub source_content: Option<String>,
}

/// Caller input for a new candidate. Representations are derived
/// deterministically by [`build_candidate_bundle`].
#[derive(Debug, Clone)]
pub struct NewCandidate {
    /// Owning project.
    pub project_id: ProjectId,
    /// Semantic classification for the proposed memory.
    pub proposed_type: MemoryType,
    /// Raw candidate body. Must be non-empty after trimming.
    pub body: String,
    /// Why the agent believes this is worth recording.
    pub rationale: Option<String>,
    /// Override the derived title. Trimmed; falls back to derived title if
    /// `None` or empty after trimming.
    pub title_override: Option<String>,
    /// Signal strength in `[0.0, 1.0]`. Clamped to the valid range.
    pub importance: f32,
    /// Agent confidence in `[0.0, 1.0]`. Clamped to the valid range.
    pub confidence: f32,
    /// Optional source provenance. Content capped at [`SOURCE_SNIPPET_MAX_BYTES`].
    pub source: Option<NewCandidateSource>,
    /// Set when this candidate duplicates an existing promoted memory.
    pub duplicate_of_memory_id: Option<MemoryId>,
    /// Set when this candidate duplicates another pending candidate.
    pub duplicate_of_candidate_id: Option<CandidateId>,
}

/// Everything the store needs to persist a candidate atomically.
///
/// Mirrors [`MemoryBundle`](crate::MemoryBundle) for the candidate table.
/// Built by [`build_candidate_bundle`]; stored atomically by
/// `Store::record_candidate` (in `vestige-store`).
#[derive(Debug, Clone)]
pub struct CandidateBundle {
    /// Fresh candidate ID — `cand_<ULID>`.
    pub id: CandidateId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Semantic classification for the proposed memory.
    pub proposed_type: MemoryType,
    /// Short display label (≤ 60 chars).
    pub title: String,
    /// First sentence of the body.
    pub one_liner: String,
    /// Full trimmed body.
    pub summary: Option<String>,
    /// Verbatim full body.
    pub full_body: String,
    /// Why the agent believes this is worth recording.
    pub rationale: Option<String>,
    /// Agent confidence, clamped to `[0.0, 1.0]`.
    pub confidence: f32,
    /// Signal strength, clamped to `[0.0, 1.0]`.
    pub importance: f32,
    /// Set when this candidate duplicates an existing promoted memory.
    pub duplicate_of_memory_id: Option<MemoryId>,
    /// Set when this candidate duplicates another pending candidate.
    pub duplicate_of_candidate_id: Option<CandidateId>,
    /// Attached sources after content-cap applied.
    pub sources: Vec<CandidateSource>,
    /// Creation timestamp (UTC).
    pub created_at: OffsetDateTime,
}

// === PUBLIC API ===

/// Build a [`CandidateBundle`] ready for `Store::record_candidate`. Pure — no I/O.
///
/// Fails with [`CoreError::Validation`] if the body is empty after trimming.
/// `importance` and `confidence` are clamped to `[0.0, 1.0]` rather than
/// rejected, matching agent-friendly ergonomics.
pub fn build_candidate_bundle(input: NewCandidate) -> Result<CandidateBundle> {
    let body_trimmed = input.body.trim();
    if body_trimmed.is_empty() {
        return Err(CoreError::Validation(
            "candidate body must not be empty".into(),
        ));
    }

    let id = CandidateId::generate();
    let now = OffsetDateTime::now_utc();

    let derived = derive(body_trimmed);

    let title = match input.title_override.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => derived.title.clone(),
    };

    let importance = input.importance.clamp(0.0, 1.0);
    let confidence = input.confidence.clamp(0.0, 1.0);

    let sources = match input.source {
        Some(src) => vec![build_candidate_source(src)],
        None => vec![],
    };

    Ok(CandidateBundle {
        id,
        project_id: input.project_id,
        proposed_type: input.proposed_type,
        title,
        one_liner: derived.one_liner,
        summary: Some(derived.summary),
        full_body: derived.full,
        rationale: input.rationale,
        confidence,
        importance,
        duplicate_of_memory_id: input.duplicate_of_memory_id,
        duplicate_of_candidate_id: input.duplicate_of_candidate_id,
        sources,
        created_at: now,
    })
}

// === PRIVATE HELPERS ===

/// Build a [`CandidateSource`] from [`NewCandidateSource`], applying the 2 KiB cap.
fn build_candidate_source(src: NewCandidateSource) -> CandidateSource {
    let (source_content, truncated) = match src.source_content.as_deref() {
        Some(raw) => {
            let (s, trunc) = truncate_at_utf8_boundary(raw, SOURCE_SNIPPET_MAX_BYTES);
            (Some(s.to_string()), trunc)
        }
        None => (None, false),
    };
    CandidateSource {
        source_type: src.source_type,
        source_ref: src.source_ref,
        source_content,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectId {
        ProjectId::from_slug("test")
    }

    fn new_candidate(body: &str) -> NewCandidate {
        NewCandidate {
            project_id: project(),
            proposed_type: MemoryType::Observation,
            body: body.to_string(),
            rationale: None,
            title_override: None,
            importance: 0.5,
            confidence: 0.8,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        }
    }

    // --- RejectionReason ---

    #[test]
    fn rejection_reason_roundtrip_known_variants() {
        for (s, variant) in [
            ("duplicate", RejectionReason::Duplicate),
            ("wrong", RejectionReason::Wrong),
            ("not_durable", RejectionReason::NotDurable),
            ("too_noisy", RejectionReason::TooNoisy),
            ("stale", RejectionReason::Stale),
        ] {
            let parsed = RejectionReason::from_str(s).unwrap();
            assert_eq!(parsed, variant);
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn rejection_reason_unknown_becomes_other() {
        let r = RejectionReason::from_str("custom reason").unwrap();
        assert_eq!(r, RejectionReason::Other("custom reason".to_string()));
        assert_eq!(r.as_str(), "custom reason");
    }

    #[test]
    fn rejection_reason_rejects_empty() {
        assert!(matches!(
            RejectionReason::from_str(""),
            Err(CoreError::InvalidRejectionReason { .. })
        ));
    }

    #[test]
    fn rejection_reason_is_case_insensitive() {
        assert_eq!(
            RejectionReason::from_str("Duplicate").unwrap(),
            RejectionReason::Duplicate
        );
        assert_eq!(
            RejectionReason::from_str("NOT_DURABLE").unwrap(),
            RejectionReason::NotDurable
        );
    }

    // --- build_candidate_bundle ---

    #[test]
    fn build_candidate_bundle_happy_path() {
        let bundle = build_candidate_bundle(new_candidate(
            "SQLite is the canonical store. No daemon needed.",
        ))
        .unwrap();
        assert!(bundle.id.as_str().starts_with("cand_"));
        assert_eq!(bundle.proposed_type, MemoryType::Observation);
        assert!(!bundle.one_liner.is_empty());
        assert!(!bundle.full_body.is_empty());
        assert!(bundle.sources.is_empty());
        assert!((bundle.confidence - 0.8).abs() < f32::EPSILON);
        assert!((bundle.importance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn build_candidate_bundle_rejects_empty_body() {
        let err = build_candidate_bundle(new_candidate("   \n  ")).unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn build_candidate_bundle_title_override() {
        let mut input = new_candidate("Some long body that would produce a different title.");
        input.title_override = Some("Custom Title".to_string());
        let bundle = build_candidate_bundle(input).unwrap();
        assert_eq!(bundle.title, "Custom Title");
    }

    #[test]
    fn build_candidate_bundle_empty_title_override_falls_back_to_derived() {
        let mut input = new_candidate("Derived title body.");
        input.title_override = Some("   ".to_string());
        let bundle = build_candidate_bundle(input).unwrap();
        assert_eq!(bundle.title, "Derived title body");
    }

    #[test]
    fn build_candidate_bundle_clamps_importance_and_confidence() {
        let mut input = new_candidate("body text");
        input.importance = 1.5;
        input.confidence = -0.3;
        let bundle = build_candidate_bundle(input).unwrap();
        assert!((bundle.importance - 1.0).abs() < f32::EPSILON);
        assert!((bundle.confidence - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn build_candidate_bundle_source_content_capped() {
        let big = "x".repeat(SOURCE_SNIPPET_MAX_BYTES + 100);
        let mut input = new_candidate("body text");
        input.source = Some(NewCandidateSource {
            source_type: "file".to_string(),
            source_ref: Some("path/to/file.rs".to_string()),
            source_content: Some(big),
        });
        let bundle = build_candidate_bundle(input).unwrap();
        assert_eq!(bundle.sources.len(), 1);
        let src = &bundle.sources[0];
        assert!(src.truncated);
        assert_eq!(
            src.source_content.as_ref().unwrap().len(),
            SOURCE_SNIPPET_MAX_BYTES
        );
    }

    #[test]
    fn build_candidate_bundle_source_not_truncated_when_small() {
        let mut input = new_candidate("body text");
        input.source = Some(NewCandidateSource {
            source_type: "clipboard".to_string(),
            source_ref: None,
            source_content: Some("small snippet".to_string()),
        });
        let bundle = build_candidate_bundle(input).unwrap();
        assert!(!bundle.sources[0].truncated);
    }
}
