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
/// representations are derived deterministically below.
#[derive(Debug, Clone)]
pub struct NewMemory<'a> {
    pub r#type: MemoryType,
    pub body: &'a str,
    pub importance: f64,
    pub source: Option<NewSource<'a>>,
}

#[derive(Debug, Clone)]
pub struct NewSource<'a> {
    pub source_type: &'a str,
    pub source_ref: Option<&'a str>,
    pub source_content: Option<&'a str>,
}

/// Everything the store needs to persist a memory atomically.
#[derive(Debug, Clone)]
pub struct MemoryBundle {
    pub memory: Memory,
    pub representations: Vec<RepresentationRow>,
    pub source: Option<SourceRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepresentationRow {
    pub memory_id: MemoryId,
    pub depth: RepresentationDepth,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRow {
    pub memory_id: MemoryId,
    pub source_type: String,
    pub source_ref: Option<String>,
    pub source_content: Option<String>,
    /// True when `source_content` was truncated to fit `SOURCE_SNIPPET_MAX_BYTES`.
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

// ========================================
// === PRIVATE HELPERS ===
// ========================================

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
