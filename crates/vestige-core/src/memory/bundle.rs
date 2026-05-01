//! Capture and persistence types — `NewMemory`, `MemoryBundle`, and the
//! `build_bundle` / `truncate_at_utf8_boundary` functions that assemble them.
//! All persistence and SQL lives in `vestige-store`; this file owns the shape.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::error::{CoreError, Result};
use crate::ids::{MemoryId, ProjectId};
use crate::representations::{depth_pick, derive, DerivedRepresentations};
use crate::types::{Memory, MemoryStatus, MemoryType, RepresentationDepth};

/// Bytes, not chars — UTF-8 boundary safe (PRD §8 source storage decision).
pub const SOURCE_SNIPPET_MAX_BYTES: usize = 2 * 1024;

const ALL_DEPTHS: [RepresentationDepth; 4] = [
    RepresentationDepth::OneLiner,
    RepresentationDepth::Summary,
    RepresentationDepth::Compressed,
    RepresentationDepth::Full,
];

/// Caller input for a new memory. The body is the raw text the user supplied;
/// representations are derived deterministically by [`build_bundle`].
#[derive(Debug, Clone)]
pub struct NewMemory<'a> {
    /// Semantic classification — drives ranking and context sections.
    pub r#type: MemoryType,
    /// Raw memory text. Must be non-empty after trimming; max recommended size
    /// is whatever fits in the `full` representation without loss.
    pub body: &'a str,
    /// Signal strength in `[0.0, 1.0]`. Validated by [`build_bundle`];
    /// returns [`CoreError::Validation`] if out of range.
    pub importance: f64,
    /// Optional source provenance. Content is capped at
    /// [`SOURCE_SNIPPET_MAX_BYTES`] by [`build_bundle`].
    pub source: Option<NewSource<'a>>,
}

/// Provenance for a new memory, passed inside [`NewMemory`].
#[derive(Debug, Clone)]
pub struct NewSource<'a> {
    /// Category of the source — e.g. `"file"`, `"url"`, `"clipboard"`.
    pub source_type: &'a str,
    /// Stable locator (file path, URL, etc.) — `None` if not applicable.
    pub source_ref: Option<&'a str>,
    /// Verbatim snippet to attach. Truncated to [`SOURCE_SNIPPET_MAX_BYTES`]
    /// (2 048 bytes) at a UTF-8 codepoint boundary before persistence.
    pub source_content: Option<&'a str>,
}

/// Everything the store needs to persist a memory atomically.
#[derive(Debug, Clone)]
pub struct MemoryBundle {
    pub memory: Memory,
    pub representations: Vec<RepresentationRow>,
    pub source: Option<SourceRow>,
}

/// One row in `memory_representations`, ready for direct SQL insertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepresentationRow {
    /// Back-reference to the owning memory.
    pub memory_id: MemoryId,
    /// Which disclosure level this row carries.
    pub depth: RepresentationDepth,
    /// Derived text content for this depth.
    pub content: String,
    /// SHA-256 of `content` (first 16 bytes, hex) — detects stale rows
    /// after a body edit without re-reading the full content.
    pub content_hash: String,
}

/// One row in `memory_sources`, ready for direct SQL insertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRow {
    /// Back-reference to the owning memory.
    pub memory_id: MemoryId,
    /// Category of the source — e.g. `"file"`, `"url"`.
    pub source_type: String,
    /// Stable locator, if provided.
    pub source_ref: Option<String>,
    /// Stored snippet (may be shorter than the original if `truncated == true`).
    pub source_content: Option<String>,
    /// `true` when `source_content` was truncated to fit [`SOURCE_SNIPPET_MAX_BYTES`].
    pub truncated: bool,
}

/// Build a bundle ready for `Store::record_memory`. Pure — no I/O.
pub fn build_bundle(project_id: &ProjectId, input: NewMemory<'_>) -> Result<MemoryBundle> {
    validate_input(&input)?;

    let now = OffsetDateTime::now_utc();
    let memory_id = MemoryId::new();
    let memory = Memory {
        id: memory_id.clone(),
        project_id: project_id.clone(),
        r#type: input.r#type,
        status: MemoryStatus::Active,
        confidence: 1.0,
        importance: input.importance,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };

    let derived = derive(input.body);
    let representations = build_representation_rows(&memory_id, &derived);

    let source = input.source.map(|s| build_source_row(&memory_id, s));

    Ok(MemoryBundle {
        memory,
        representations,
        source,
    })
}

/// Truncate `s` to fit within `max_bytes`, never splitting a UTF-8 codepoint.
/// Returns `(slice, was_truncated)`.
pub fn truncate_at_utf8_boundary(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    (&s[..cut], true)
}

// === PRIVATE HELPERS ===

/// Build all four [`RepresentationRow`]s from a freshly derived set of representations.
fn build_representation_rows(
    id: &MemoryId,
    derived: &DerivedRepresentations,
) -> Vec<RepresentationRow> {
    ALL_DEPTHS
        .iter()
        .map(|d| {
            let content = depth_pick(*d, derived).to_string();
            let content_hash = hash(&content);
            RepresentationRow {
                memory_id: id.clone(),
                depth: *d,
                content,
                content_hash,
            }
        })
        .collect()
}

/// Build a [`SourceRow`] from [`NewSource`] input, applying the 2 KiB content cap.
fn build_source_row(id: &MemoryId, src: NewSource<'_>) -> SourceRow {
    let (content, truncated) = match src.source_content {
        Some(raw) => {
            let (s, trunc) = truncate_at_utf8_boundary(raw, SOURCE_SNIPPET_MAX_BYTES);
            (Some(s.to_string()), trunc)
        }
        None => (None, false),
    };
    SourceRow {
        memory_id: id.clone(),
        source_type: src.source_type.to_string(),
        source_ref: src.source_ref.map(str::to_string),
        source_content: content,
        truncated,
    }
}

/// Validate [`NewMemory`] fields before building a bundle.
fn validate_input(input: &NewMemory<'_>) -> Result<()> {
    if input.body.trim().is_empty() {
        return Err(CoreError::Validation(
            "memory body must not be empty".into(),
        ));
    }
    if !(0.0..=1.0).contains(&input.importance) {
        return Err(CoreError::Validation(format!(
            "importance must be in [0.0, 1.0], got {}",
            input.importance
        )));
    }
    Ok(())
}

/// SHA-256 of `s`, truncated to the first 16 bytes, hex-encoded (32 chars).
fn hash(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    hex::encode(&digest[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectId {
        ProjectId::from_slug("test")
    }

    #[test]
    fn build_bundle_creates_four_representations() {
        let bundle = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Decision,
                body: "Use SQLite as canonical store. Vector indexes are replaceable.",
                importance: 0.8,
                source: None,
            },
        )
        .unwrap();
        assert_eq!(bundle.representations.len(), 4);
        let depths: Vec<_> = bundle.representations.iter().map(|r| r.depth).collect();
        assert!(depths.contains(&RepresentationDepth::OneLiner));
        assert!(depths.contains(&RepresentationDepth::Full));
        assert!(bundle.source.is_none());
        assert_eq!(bundle.memory.r#type, MemoryType::Decision);
        assert_eq!(bundle.memory.status, MemoryStatus::Active);
    }

    #[test]
    fn rejects_empty_body() {
        let err = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Note,
                body: "   \n",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn rejects_out_of_range_importance() {
        let err = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Note,
                body: "anything",
                importance: 1.5,
                source: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // A 3-byte UTF-8 char repeated; cap mid-char.
        let s = "★".repeat(10); // 30 bytes
        let (cut, truncated) = truncate_at_utf8_boundary(&s, 7);
        assert!(truncated);
        // Should land on a char boundary: 6 bytes = 2 stars.
        assert_eq!(cut.chars().count(), 2);
        assert!(s.starts_with(cut));
    }

    #[test]
    fn truncate_passthrough_when_under_limit() {
        let (cut, truncated) = truncate_at_utf8_boundary("hello", 100);
        assert!(!truncated);
        assert_eq!(cut, "hello");
    }

    #[test]
    fn source_snippet_capped() {
        let big = "x".repeat(SOURCE_SNIPPET_MAX_BYTES + 100);
        let bundle = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Observation,
                body: "anything",
                importance: 0.5,
                source: Some(NewSource {
                    source_type: "file",
                    source_ref: Some("path/to/file.rs"),
                    source_content: Some(&big),
                }),
            },
        )
        .unwrap();
        let src = bundle.source.unwrap();
        assert!(src.truncated);
        assert_eq!(src.source_content.unwrap().len(), SOURCE_SNIPPET_MAX_BYTES);
    }
}
