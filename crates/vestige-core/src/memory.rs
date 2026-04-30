//! Memory engine — pure functions that build persistable bundles from user
//! input and project bundles back into agent-friendly cards / details.
//!
//! All persistence and SQL lives in `vestige-store`. This module owns the
//! shape of the data and the derivation rules.

use std::collections::HashMap;
use std::str::FromStr;

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

/// Which retrieval strategy to use for a search request.
///
/// `Lexical` uses FTS5 only (default, always available).
/// `Semantic` uses vector nearest-neighbours only (requires embeddings).
/// `Hybrid` merges both sides via [`merge_hits`] with [`HybridOpts`] weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Lexical,
    Semantic,
    Hybrid,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

impl FromStr for SearchMode {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "lexical" => Ok(Self::Lexical),
            "semantic" => Ok(Self::Semantic),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(CoreError::Validation(format!(
                "unknown search mode \"{other}\"; expected one of: lexical, semantic, hybrid"
            ))),
        }
    }
}

/// Broken-down score produced by [`merge_hits`] for hybrid and semantic paths.
///
/// All component scores are in [0, 1] (or close to it for type_boost).
/// `total` is the weighted sum computed with the caller-supplied [`HybridOpts`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridScore {
    /// Normalised FTS bm25 contribution ([0, 1]).
    pub fts: f64,
    /// Normalised cosine similarity contribution ([0, 1]).
    pub vector: f64,
    /// Importance contribution (importance is already [0, 1] in this schema).
    pub importance: f64,
    /// Memory-type boost (see [`type_boost`]).
    pub type_boost: f64,
    /// Weighted total: `fts_w*fts + vec_w*vector + imp_w*importance + type_w*type_boost`.
    pub total: f64,
}

/// A semantic search result from the store layer.
///
/// This is the core-side twin of `vestige_store::embeddings::VectorHit`.
/// Callers in `vestige-store` must map `VectorHit → SemanticHit` at the use
/// site to preserve the one-way dependency (`store` → `core`, never the reverse).
#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub memory_id: MemoryId,
    /// `"summary"` | `"compressed"` | etc. — the representation that was embedded.
    pub representation_type: String,
    /// Cosine similarity in [-1, 1]. Typically [0, 1] for L2-normalised vectors.
    pub similarity: f64,
}

/// Weights and result size for hybrid score merging.
///
/// Default weights follow PRD §11.1 and sum to 1.0.
#[derive(Debug, Clone)]
pub struct HybridOpts {
    /// Weight for normalised FTS score. Default 0.55.
    pub fts_weight: f64,
    /// Weight for normalised cosine score. Default 0.35.
    pub vector_weight: f64,
    /// Weight for memory importance (already [0, 1]). Default 0.07.
    pub importance_weight: f64,
    /// Weight for memory-type boost. Default 0.03.
    pub type_weight: f64,
    /// Maximum results to return after merging. Default 8.
    pub limit: u32,
}

impl Default for HybridOpts {
    fn default() -> Self {
        Self {
            fts_weight: 0.55,
            vector_weight: 0.35,
            importance_weight: 0.07,
            type_weight: 0.03,
            limit: 8,
        }
    }
}

/// Filter passed to `search_memories`.
///
/// New fields (`mode`, `include_score_parts`) default to the V0 lexical-only
/// behaviour so existing call sites that use struct-literal initialisation with
/// `..Default::default()` continue to compile unchanged.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    pub r#type: Option<MemoryType>,
    pub limit: Option<u32>,
    /// Search strategy. Default: `SearchMode::Lexical` (FTS5 only).
    pub mode: SearchMode,
    /// When `true`, [`ScoredCard::score_parts`] will be populated by the hybrid
    /// merge path. The lexical-only path always leaves it `None`.
    pub include_score_parts: bool,
}

/// Raw search result from the store: a fetched memory plus the best matching
/// representation's bm25 score (lower = better, as SQLite returns it).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub fetched: FetchedMemory,
    pub bm25: f64,
}

/// A search result projected for display: compact card + composite score.
///
/// `score_parts` is `Some` only when hybrid/semantic merging was performed
/// with `include_score_parts = true` in the [`SearchFilter`]. The lexical-only
/// path (`rank_hits`) always sets it to `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredCard {
    #[serde(flatten)]
    pub card: MemoryCard,
    pub score: f64,
    /// Broken-down score components — populated by [`merge_hits`], absent for
    /// lexical-only results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_parts: Option<HybridScore>,
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
/// (highest first). `score_parts` is always `None` on this path — use
/// [`merge_hits`] for the hybrid path with full diagnostics.
pub fn rank_hits(hits: Vec<SearchHit>) -> Vec<ScoredCard> {
    let now = OffsetDateTime::now_utc();
    let mut scored: Vec<ScoredCard> = hits
        .into_iter()
        .map(|hit| {
            let score = composite_score(&hit, now);
            ScoredCard {
                card: project_card(&hit.fetched),
                score,
                score_parts: None,
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
// === HYBRID SCORING ===
// ========================================

/// Normalise a slice of [`SearchHit`] bm25 scores into a `MemoryId → [0, 1]` map.
///
/// BM25 from SQLite is negative-valued (lower = better match). We flip the sign
/// and apply min-max normalisation across the candidate set so higher = better.
///
/// Edge cases:
/// - Empty input → empty map (not NaN).
/// - All scores equal → every entry maps to `0.5` (not NaN).
///
/// Call this before [`merge_hits`]; the result feeds the `fts_scores` parameter.
pub fn normalise_fts(hits: &[SearchHit]) -> HashMap<MemoryId, f64> {
    if hits.is_empty() {
        return HashMap::new();
    }

    // Flip sign: higher raw = better match.
    let raw: Vec<f64> = hits.iter().map(|h| -h.bm25).collect();
    let min = raw.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = raw.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;

    hits.iter()
        .zip(raw)
        .map(|(hit, r)| {
            let norm = if range == 0.0 {
                // All scores identical — no signal to differentiate them.
                // Midpoint (0.5) avoids over-rewarding a solitary lexical hit
                // in hybrid: mapping to 1.0 would dominate the merged total.
                0.5
            } else {
                (r - min) / range
            };
            (hit.fetched.memory.id.clone(), norm)
        })
        .collect()
}

/// Normalise a slice of [`SemanticHit`] cosine similarities into a
/// `MemoryId → [0, 1]` map.
///
/// When a memory has multiple semantic hits (e.g. summary + compressed
/// representations), **only the highest similarity** is kept in the map.
///
/// Negative cosine similarity is clamped to `0.0` (irrelevant result).
///
/// Edge cases:
/// - Empty input → empty map (not NaN).
///
/// Call this before [`merge_hits`]; the result feeds the `vector_scores` parameter.
pub fn normalise_cosine(hits: &[SemanticHit]) -> HashMap<MemoryId, f64> {
    let mut map: HashMap<MemoryId, f64> = HashMap::new();
    for hit in hits {
        let clamped = hit.similarity.clamp(0.0, 1.0);
        let entry = map.entry(hit.memory_id.clone()).or_insert(0.0);
        if clamped > *entry {
            *entry = clamped;
        }
    }
    map
}

/// Merge pre-normalised FTS and vector scores for a hydrated candidate set into
/// ranked [`ScoredCard`]s.
///
/// # Caller contract
///
/// - `candidates` must be a **unified** set of [`SearchHit`]s covering every
///   memory that appeared in either the lexical or semantic result sets.
///   For semantic-only memories the caller must fetch the full memory from the
///   store and synthesise a `SearchHit` with `bm25 = 0.0` (which maps to the
///   lowest normalised FTS score, or simply won't appear in `fts_scores`).
/// - `fts_scores` and `vector_scores` are pre-normalised to [0, 1] — use
///   [`normalise_fts`] and [`normalise_cosine`] to produce them.
/// - Memories absent from a score map receive a score of `0.0` for that
///   component (i.e. they get no contribution from that side).
///
/// # Output
///
/// Results are sorted by `total` descending and truncated to `opts.limit`.
/// `score_parts` is always populated on the returned cards.
pub fn merge_hits(
    candidates: Vec<SearchHit>,
    fts_scores: &HashMap<MemoryId, f64>,
    vector_scores: &HashMap<MemoryId, f64>,
    opts: &HybridOpts,
) -> Vec<ScoredCard> {
    let mut scored: Vec<ScoredCard> = candidates
        .into_iter()
        .map(|hit| {
            let mid = &hit.fetched.memory.id;
            let fts = fts_scores.get(mid).copied().unwrap_or(0.0);
            let vector = vector_scores.get(mid).copied().unwrap_or(0.0);
            let importance = hit.fetched.memory.importance.clamp(0.0, 1.0);
            let type_b = type_boost(hit.fetched.memory.r#type);
            let total = opts.fts_weight * fts
                + opts.vector_weight * vector
                + opts.importance_weight * importance
                + opts.type_weight * type_b;

            let parts = HybridScore {
                fts,
                vector,
                importance,
                type_boost: type_b,
                total,
            };

            ScoredCard {
                card: project_card(&hit.fetched),
                score: total,
                score_parts: Some(parts),
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(opts.limit as usize);
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

/// Per-type boost for hybrid ranking (PRD §11.2).
///
/// Returns a value roughly in [0, 1] that biases the hybrid total toward
/// high-signal memory types like `ProjectSummary` and `Decision`.
fn type_boost(t: MemoryType) -> f64 {
    match t {
        MemoryType::ProjectSummary => 1.0,
        MemoryType::Decision => 0.8,
        MemoryType::Preference => 0.6,
        MemoryType::OpenQuestion => 0.6,
        MemoryType::Note | MemoryType::Observation => 0.5,
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

    // ----------------------------------------
    // === SEARCH MODE ===
    // ----------------------------------------

    #[test]
    fn search_mode_round_trip() {
        assert_eq!(SearchMode::Lexical.as_str(), "lexical");
        assert_eq!(SearchMode::Semantic.as_str(), "semantic");
        assert_eq!(SearchMode::Hybrid.as_str(), "hybrid");

        assert_eq!(
            SearchMode::from_str("lexical").unwrap(),
            SearchMode::Lexical
        );
        assert_eq!(
            SearchMode::from_str("LEXICAL").unwrap(),
            SearchMode::Lexical
        );
        assert_eq!(
            SearchMode::from_str("Semantic").unwrap(),
            SearchMode::Semantic
        );
        assert_eq!(SearchMode::from_str("HYBRID").unwrap(), SearchMode::Hybrid);

        let err = SearchMode::from_str("fuzzy").unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    // ----------------------------------------
    // === HYBRID OPTS ===
    // ----------------------------------------

    #[test]
    fn hybrid_opts_default_weights_sum_to_one() {
        let opts = HybridOpts::default();
        let sum = opts.fts_weight + opts.vector_weight + opts.importance_weight + opts.type_weight;
        assert!(
            (sum - 1.0).abs() < f64::EPSILON * 4.0,
            "weights sum {sum} ≠ 1.0"
        );
        assert_eq!(opts.limit, 8);
    }

    // ----------------------------------------
    // === TYPE BOOST ===
    // ----------------------------------------

    #[test]
    fn type_boost_table() {
        assert!(
            type_boost(MemoryType::ProjectSummary) > type_boost(MemoryType::Decision),
            "project_summary > decision"
        );
        assert!(
            type_boost(MemoryType::Decision) > type_boost(MemoryType::Note),
            "decision > note"
        );
        assert_eq!(
            type_boost(MemoryType::Note),
            type_boost(MemoryType::Observation)
        );
    }

    // ----------------------------------------
    // === NORMALISE HELPERS ===
    // ----------------------------------------

    #[test]
    fn normalise_fts_handles_empty_and_constant() {
        // Empty → empty map, no NaN.
        let map = normalise_fts(&[]);
        assert!(map.is_empty());

        // All-equal bm25 → every entry is 0.5 (midpoint fallback, not NaN).
        let project = project();
        let make_hit = |bm25: f64| -> SearchHit {
            let bundle = build_bundle(
                &project,
                NewMemory {
                    r#type: MemoryType::Note,
                    body: "test",
                    importance: 0.5,
                    source: None,
                },
            )
            .unwrap();
            SearchHit {
                fetched: FetchedMemory {
                    memory: bundle.memory,
                    representations: bundle.representations,
                    sources: vec![],
                },
                bm25,
            }
        };

        let hits = vec![make_hit(-5.0), make_hit(-5.0)];
        let map = normalise_fts(&hits);
        assert_eq!(map.len(), 2);
        for v in map.values() {
            assert!((v - 0.5).abs() < f64::EPSILON, "expected 0.5, got {v}");
        }
    }

    #[test]
    fn normalise_cosine_clamps_negative_to_zero() {
        let id = MemoryId::new();
        let hits = vec![SemanticHit {
            memory_id: id.clone(),
            representation_type: "summary".to_string(),
            similarity: -0.3,
        }];
        let map = normalise_cosine(&hits);
        assert_eq!(map[&id], 0.0);
    }

    // ----------------------------------------
    // === MERGE HITS ===
    // ----------------------------------------

    fn make_search_hit(memory_type: MemoryType, importance: f64, bm25: f64) -> SearchHit {
        let bundle = build_bundle(
            &project(),
            NewMemory {
                r#type: memory_type,
                body: "a test memory body",
                importance,
                source: None,
            },
        )
        .unwrap();
        SearchHit {
            fetched: FetchedMemory {
                memory: bundle.memory,
                representations: bundle.representations,
                sources: vec![],
            },
            bm25,
        }
    }

    #[test]
    fn merge_hits_lexical_only() {
        let hit = make_search_hit(MemoryType::Decision, 0.8, -8.0);
        let id = hit.fetched.memory.id.clone();
        let fts_scores = normalise_fts(std::slice::from_ref(&hit));
        let vector_scores = HashMap::new();
        let opts = HybridOpts::default();
        let results = merge_hits(vec![hit], &fts_scores, &vector_scores, &opts);
        assert_eq!(results.len(), 1);
        let parts = results[0].score_parts.as_ref().unwrap();
        assert_eq!(parts.vector, 0.0, "no vector contribution");
        assert!(parts.fts > 0.0 || fts_scores[&id] == 0.0);
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn merge_hits_semantic_only() {
        let hit = make_search_hit(MemoryType::Note, 0.5, 0.0);
        let id = hit.fetched.memory.id.clone();
        let fts_scores = HashMap::new();
        let vector_scores = {
            let mut m = HashMap::new();
            m.insert(id, 0.9_f64);
            m
        };
        let opts = HybridOpts::default();
        let results = merge_hits(vec![hit], &fts_scores, &vector_scores, &opts);
        assert_eq!(results.len(), 1);
        let parts = results[0].score_parts.as_ref().unwrap();
        assert_eq!(parts.fts, 0.0, "no fts contribution");
        assert!((parts.vector - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_hits_dedup() {
        // Same memory appears once in candidates but in both score maps.
        let hit = make_search_hit(MemoryType::Decision, 0.9, -10.0);
        let id = hit.fetched.memory.id.clone();
        let fts_scores = {
            let mut m = HashMap::new();
            m.insert(id.clone(), 0.8);
            m
        };
        let vector_scores = {
            let mut m = HashMap::new();
            m.insert(id, 0.7);
            m
        };
        let opts = HybridOpts::default();
        let results = merge_hits(vec![hit], &fts_scores, &vector_scores, &opts);
        // Should be exactly one row.
        assert_eq!(results.len(), 1);
        let parts = results[0].score_parts.as_ref().unwrap();
        assert!((parts.fts - 0.8).abs() < f64::EPSILON);
        assert!((parts.vector - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_hits_sort_and_limit() {
        // 5 hits with clearly different vector scores; limit = 3.
        let scores = [0.1, 0.9, 0.5, 0.3, 0.7];
        let hits: Vec<SearchHit> = scores
            .iter()
            .map(|_| make_search_hit(MemoryType::Note, 0.5, 0.0))
            .collect();

        let vector_scores: HashMap<MemoryId, f64> = hits
            .iter()
            .zip(scores.iter())
            .map(|(h, &s)| (h.fetched.memory.id.clone(), s))
            .collect();
        let fts_scores = HashMap::new();
        let opts = HybridOpts {
            limit: 3,
            ..HybridOpts::default()
        };
        let results = merge_hits(hits, &fts_scores, &vector_scores, &opts);
        assert_eq!(results.len(), 3);
        // Top 3 expected vector scores: 0.9, 0.7, 0.5 — verify descending order.
        for window in results.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "results not sorted desc: {} < {}",
                window[0].score,
                window[1].score
            );
        }
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
