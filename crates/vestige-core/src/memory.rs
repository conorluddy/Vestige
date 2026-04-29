//! Memory engine — pure functions that build persistable bundles from user
//! input and project bundles back into agent-friendly cards / details.
//!
//! All persistence and SQL lives in `vestige-store`. This module owns the
//! shape of the data and the derivation rules.

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

/// Compact card returned from list/search — agents expand on demand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCard {
    pub id: MemoryId,
    pub r#type: MemoryType,
    pub status: MemoryStatus,
    pub title: String,
    pub one_liner: String,
    pub importance: f64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub available_depths: Vec<RepresentationDepth>,
}

/// Full detail used by `vestige show` and `vestige_expand`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDetail {
    pub card: MemoryCard,
    pub representations: Vec<(RepresentationDepth, String)>,
    pub sources: Vec<SourceRow>,
}

/// What the store returns after fetching a memory + its joined rows.
#[derive(Debug, Clone)]
pub struct FetchedMemory {
    pub memory: Memory,
    pub representations: Vec<RepresentationRow>,
    pub sources: Vec<SourceRow>,
}

/// Filter passed to `list_memories`.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub include_deleted: bool,
    pub r#type: Option<MemoryType>,
    pub limit: Option<u32>,
}

/// Filter passed to `search_memories`.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    pub r#type: Option<MemoryType>,
    pub limit: Option<u32>,
}

/// Raw search result from the store: a fetched memory plus the best matching
/// representation's bm25 score (lower = better, as SQLite returns it).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub fetched: FetchedMemory,
    pub bm25: f64,
}

/// A search result projected for display: compact card + composite score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredCard {
    #[serde(flatten)]
    pub card: MemoryCard,
    pub score: f64,
}

/// Sanitize a free-text query for FTS5 MATCH. Collapses to alphanumeric
/// tokens (plus `-` and `_`), joined by whitespace (FTS5 implicit AND with
/// the porter stemmer doing the rest). Returns empty string when the query
/// has no usable tokens — callers should skip the search in that case.
pub fn sanitize_fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ========================================
// === RANKING ===
// ========================================

/// Composite ranking from PRD §14.2:
///
/// ```text
/// score = fts_norm + 0.3 * importance + type_boost + recency_boost
/// ```
///
/// Where:
/// * `fts_norm = -bm25 / 10.0` (SQLite returns negative bm25 with lower =
///   better; flipping makes higher = better in a roughly [0, 3] range).
/// * `type_boost`: decisions and project_summary get +0.15 each.
/// * `recency_boost = 0.2 * exp(-days_since_updated / 30.0)`.
pub fn composite_score(hit: &SearchHit, now: OffsetDateTime) -> f64 {
    let fts_norm = (-hit.bm25) / 10.0;
    let importance_term = 0.3 * hit.fetched.memory.importance;
    let type_boost = match hit.fetched.memory.r#type {
        MemoryType::Decision | MemoryType::ProjectSummary => 0.15,
        _ => 0.0,
    };
    let age = now - hit.fetched.memory.updated_at;
    let days = (age.whole_seconds() as f64) / 86_400.0;
    let recency_boost = 0.2 * (-(days.max(0.0)) / 30.0).exp();
    fts_norm + importance_term + type_boost + recency_boost
}

/// Project a list of search hits into ScoredCards, sorted by composite score
/// (highest first).
pub fn rank_hits(hits: Vec<SearchHit>) -> Vec<ScoredCard> {
    let now = OffsetDateTime::now_utc();
    let mut scored: Vec<ScoredCard> = hits
        .into_iter()
        .map(|hit| {
            let score = composite_score(&hit, now);
            ScoredCard {
                card: project_card(&hit.fetched),
                score,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

// ========================================
// === BUNDLE BUILDING ===
// ========================================

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
// === PROJECTION (rows → cards / details) ===
// ========================================

pub fn project_card(fetched: &FetchedMemory) -> MemoryCard {
    let title = pick_representation(fetched, RepresentationDepth::OneLiner)
        .map(|r| derive_title_from_one_liner(&r.content))
        .unwrap_or_default();
    let one_liner = pick_representation(fetched, RepresentationDepth::OneLiner)
        .map(|r| r.content.clone())
        .unwrap_or_default();

    MemoryCard {
        id: fetched.memory.id.clone(),
        r#type: fetched.memory.r#type,
        status: fetched.memory.status,
        title,
        one_liner,
        importance: fetched.memory.importance,
        created_at: fetched.memory.created_at,
        updated_at: fetched.memory.updated_at,
        available_depths: fetched.representations.iter().map(|r| r.depth).collect(),
    }
}

pub fn project_detail(fetched: &FetchedMemory) -> MemoryDetail {
    let card = project_card(fetched);
    let representations = fetched
        .representations
        .iter()
        .map(|r| (r.depth, r.content.clone()))
        .collect();
    let sources = fetched.sources.clone();
    MemoryDetail {
        card,
        representations,
        sources,
    }
}

pub fn pick_representation(
    fetched: &FetchedMemory,
    depth: RepresentationDepth,
) -> Option<&RepresentationRow> {
    fetched.representations.iter().find(|r| r.depth == depth)
}

fn derive_title_from_one_liner(one_liner: &str) -> String {
    // The one-liner is already short enough by construction (first sentence).
    // Re-using `derive` would re-enter the title-truncation rule, so keep it
    // direct here — same MAX as `representations::derive`.
    const MAX: usize = 60;
    if one_liner.chars().count() <= MAX {
        return one_liner.to_string();
    }
    let mut out = String::new();
    let mut count = 0usize;
    for word in one_liner.split_whitespace() {
        let prospective = count + word.chars().count() + if out.is_empty() { 0 } else { 1 };
        if prospective > MAX {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        count = prospective;
    }
    if out.is_empty() {
        out.extend(one_liner.chars().take(MAX));
    }
    out
}

// ========================================
// === TESTS ===
// ========================================

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
    fn sanitize_strips_fts_specials() {
        assert_eq!(sanitize_fts_query("MCP adapter!"), "MCP adapter");
        assert_eq!(
            sanitize_fts_query("  (foo) \"bar\" baz-qux "),
            "foo bar baz-qux"
        );
        assert_eq!(sanitize_fts_query("***"), "");
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn ranking_boosts_decisions_over_notes_at_equal_match() {
        // Two memories with identical bm25 + importance + recency — the
        // decision should come out ahead via type boost.
        let now = OffsetDateTime::now_utc();
        let project = project();
        let bundle_d = build_bundle(
            &project,
            NewMemory {
                r#type: MemoryType::Decision,
                body: "Use SQLite",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        let bundle_n = build_bundle(
            &project,
            NewMemory {
                r#type: MemoryType::Note,
                body: "Use SQLite",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        let hit_d = SearchHit {
            fetched: FetchedMemory {
                memory: bundle_d.memory,
                representations: bundle_d.representations,
                sources: vec![],
            },
            bm25: -10.0,
        };
        let hit_n = SearchHit {
            fetched: FetchedMemory {
                memory: bundle_n.memory,
                representations: bundle_n.representations,
                sources: vec![],
            },
            bm25: -10.0,
        };
        assert!(composite_score(&hit_d, now) > composite_score(&hit_n, now));
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
