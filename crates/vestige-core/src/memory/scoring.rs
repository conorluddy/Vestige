//! Ranking and hybrid merge — `ScoredCard`, `composite_score`, `rank_hits`,
//! `normalise_fts`, `normalise_cosine`, and `merge_hits`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::MemoryId;
use crate::types::MemoryType;

use super::projection::{project_card, MemoryCard};
use super::search::{HybridOpts, SearchHit, SemanticHit};

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
    /// Memory-type boost (private `type_boost` helper).
    pub type_boost: f64,
    /// Recall-count usage boost, already scaled to [0, 1] by dividing out
    /// [`usage_boost`]'s 0.1 ceiling so every component shares one range.
    pub usage: f64,
    /// Weighted total:
    /// `fts_w*fts + vec_w*vector + imp_w*importance + type_w*type_boost + usage_w*usage`.
    pub total: f64,
}

/// A search result projected for display: compact card + composite score.
///
/// `score_parts` is populated on the hybrid and semantic paths and is `None`
/// on the lexical-only path (`rank_hits`). On semantic-only the `vector` and
/// `total` components both equal the displayed `score`; the other components
/// are zero. On hybrid the breakdown is the genuine weighted decomposition
/// produced by [`merge_hits`].
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

/// Diminishing-returns boost for how often a memory has been recalled (#133).
///
/// ```text
/// usage_boost(n) = 0.1 * (1 - exp(-n / 5))
/// ```
///
/// Asymptotic to `0.1`, so a heavily-recalled memory can never dominate
/// relevance — being used often is a tie-breaker, not a trump card. The knee
/// sits around five recalls (`usage_boost(5) ≈ 0.063`), which is where a
/// memory has plausibly proven itself rather than been hit once by chance.
///
/// `recall_count` is clamped at zero before use. It is `i64` to mirror
/// SQLite's signed storage, and a negative value would make `1 - exp(+x)`
/// negative — silently inverting the term into a penalty. Large values
/// underflow to `0.0` cleanly, so there is no `NaN`/`Inf` path.
///
/// Note the boost saturates numerically: for `n >= 190`, `exp(-n/5)` falls
/// below f64's ULP relative to `1.0` and the result is exactly `0.1`.
pub fn usage_boost(recall_count: i64) -> f64 {
    let n = recall_count.max(0) as f64;
    0.1 * (1.0 - (-n / 5.0).exp())
}

/// Composite ranking from PRD §14.2:
///
/// ```text
/// score = fts_norm + 0.3 * importance + type_boost + recency_boost + usage_boost
/// ```
///
/// Where:
/// * `fts_norm = -bm25 / 10.0` (SQLite returns negative bm25 with lower =
///   better; flipping makes higher = better in a roughly [0, 3] range).
/// * `type_boost`: decisions and project_summary get +0.15 each.
/// * `recency_boost = 0.2 * exp(-days_since_updated / 30.0)`.
/// * `usage_boost`: see [`usage_boost`] — bounded at 0.1.
pub fn composite_score(hit: &SearchHit, now: OffsetDateTime) -> f64 {
    composite_score_with(hit, now, UsageWeighting::Include)
}

/// Whether a scoring pass counts the recall-usage term.
///
/// Live search uses [`UsageWeighting::Include`]. Trace replay uses
/// [`UsageWeighting::Exclude`], because searching *increments* the counter
/// that scoring reads: a search records its scores, then bumps `recall_count`,
/// so replaying that trace against a completely unchanged corpus would
/// otherwise report a score change caused by the original search itself.
/// Replay exists to detect drift in the corpus, not drift in how often the
/// corpus has been read, so it scores on the stable terms only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageWeighting {
    /// Count the usage term — live search ranking.
    Include,
    /// Ignore the usage term — reproducible replay comparison.
    Exclude,
}

/// [`composite_score`] with explicit control over the usage term.
pub fn composite_score_with(
    hit: &SearchHit,
    now: OffsetDateTime,
    usage_weighting: UsageWeighting,
) -> f64 {
    let fts_norm = (-hit.bm25) / 10.0;
    let importance_term = 0.3 * hit.fetched.memory.importance;
    let type_boost = match hit.fetched.memory.r#type {
        MemoryType::Decision | MemoryType::ProjectSummary => 0.15,
        _ => 0.0,
    };
    let age = now - hit.fetched.memory.updated_at;
    let days = (age.whole_seconds() as f64) / 86_400.0;
    let recency_boost = 0.2 * (-(days.max(0.0)) / 30.0).exp();
    let usage = match usage_weighting {
        UsageWeighting::Include => usage_boost(hit.fetched.memory.recall_count),
        UsageWeighting::Exclude => 0.0,
    };
    fts_norm + importance_term + type_boost + recency_boost + usage
}

/// Project a list of search hits into ScoredCards, sorted by composite score
/// (highest first). `score_parts` is always `None` on this path — use
/// [`merge_hits`] for the hybrid path with full diagnostics.
pub fn rank_hits(hits: Vec<SearchHit>) -> Vec<ScoredCard> {
    rank_hits_with(hits, UsageWeighting::Include)
}

/// [`rank_hits`] with explicit control over the usage term — see
/// [`UsageWeighting`].
pub fn rank_hits_with(hits: Vec<SearchHit>, usage_weighting: UsageWeighting) -> Vec<ScoredCard> {
    let now = OffsetDateTime::now_utc();
    let mut scored: Vec<ScoredCard> = hits
        .into_iter()
        .map(|hit| {
            let score = composite_score_with(&hit, now, usage_weighting);
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
            // Rescale to [0, 1] so every component in the weighted sum shares
            // one range — `usage_boost` is capped at 0.1 for the additive
            // `composite_score` path, which would otherwise make this leg
            // contribute an order of magnitude less than its weight implies.
            let usage = usage_boost(hit.fetched.memory.recall_count) * 10.0;
            let total = opts.fts_weight * fts
                + opts.vector_weight * vector
                + opts.importance_weight * importance
                + opts.type_weight * type_b
                + opts.usage_weight * usage;

            let parts = HybridScore {
                fts,
                vector,
                importance,
                type_boost: type_b,
                usage,
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
// === PRIVATE HELPERS ===
// ========================================

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProjectId;
    use crate::memory::bundle::{build_bundle, NewMemory};
    use crate::memory::projection::FetchedMemory;
    use crate::types::MemoryType;

    fn project() -> ProjectId {
        ProjectId::from_slug("test")
    }

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
    fn usage_boost_is_zero_at_zero_and_asymptotic_below_the_cap() {
        assert_eq!(usage_boost(0), 0.0, "never-recalled memory gets no boost");

        let five = usage_boost(5);
        assert!(
            (five - 0.063_212).abs() < 1e-6,
            "usage_boost(5) should be ≈0.0632, got {five}"
        );

        // 50 rather than a larger value on purpose: for n >= 190, exp(-n/5)
        // underflows below f64's ULP relative to 1.0, so the result is
        // *exactly* 0.1 and a strict `< 0.1` assertion fails. 50 still proves
        // the curve is bounded while staying numerically honest.
        let fifty = usage_boost(50);
        assert!(fifty < 0.1, "usage_boost must stay under the 0.1 cap");
        assert!(fifty > 0.099, "usage_boost(50) should be near the cap");
    }

    #[test]
    fn usage_boost_clamps_negative_recall_counts() {
        // recall_count is i64 to mirror SQLite storage; a negative value would
        // make 1 - exp(+x) negative and silently turn the boost into a
        // penalty. Clamping means a corrupt counter costs nothing rather than
        // inverting the ranking.
        assert_eq!(usage_boost(-1), 0.0);
        assert_eq!(usage_boost(i64::MIN), 0.0);
    }

    #[test]
    fn frequently_recalled_memory_outranks_identical_unused_one() {
        let now = OffsetDateTime::now_utc();

        let mut used = make_search_hit(MemoryType::Note, 0.5, -5.0);
        used.fetched.memory.recall_count = 10;
        let unused = make_search_hit(MemoryType::Note, 0.5, -5.0);

        assert!(
            composite_score(&used, now) > composite_score(&unused, now),
            "recall_count 10 must rank strictly above 0 when all else is equal"
        );
    }

    #[test]
    fn merge_hits_ranks_recalled_memory_above_identical_unused_one() {
        let mut used = make_search_hit(MemoryType::Note, 0.5, -5.0);
        used.fetched.memory.recall_count = 10;
        let unused = make_search_hit(MemoryType::Note, 0.5, -5.0);
        let used_id = used.fetched.memory.id.clone();

        // Identical retrieval scores on both legs — usage is the only
        // differentiator left.
        let mut fts = HashMap::new();
        fts.insert(used_id.clone(), 0.5);
        fts.insert(unused.fetched.memory.id.clone(), 0.5);

        let ranked = merge_hits(
            vec![unused, used],
            &fts,
            &HashMap::new(),
            &HybridOpts::default(),
        );

        assert_eq!(
            ranked[0].card.id, used_id,
            "the recalled memory must sort first"
        );
        assert!(
            ranked[0].score_parts.as_ref().unwrap().usage > 0.0,
            "usage component must be populated in the breakdown"
        );
        assert_eq!(
            ranked[1].score_parts.as_ref().unwrap().usage,
            0.0,
            "unused memory's usage component must be zero"
        );
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
}
