//! Query types and FTS query preparation — `SearchMode`, `SearchFilter`,
//! `SearchHit`, `SemanticHit`, `HybridOpts`, `ListFilter`, and the
//! `sanitize_fts_query` / `resolve_default_mode` helpers.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::MemoryId;
use crate::types::MemoryType;

use super::projection::FetchedMemory;

/// Which retrieval strategy to use for a search request.
///
/// `Lexical` uses FTS5 only (default, always available).
/// `Semantic` uses vector nearest-neighbours only (requires embeddings).
/// `Hybrid` merges both sides via [`crate::merge_hits`] with [`HybridOpts`] weights.
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

/// Resolve the active search mode from an explicit user choice and an optional
/// config default, applying the canonical precedence chain:
///
/// 1. `explicit` — the value of `--mode` or an alias flag (`--lexical` etc.)
///    after the caller has already converted alias flags to their string form.
/// 2. `config_default` — `[search] default_mode` from `.vestige/config.toml`.
/// 3. [`SearchMode::Lexical`] — the unconditional fallback.
///
/// Both inputs are raw `&str` slices so this function has no dependency on
/// `rusqlite`, `clap`, or `rmcp` — it belongs in `vestige-core` and callers
/// from the CLI, MCP, or any future transport can share it.
///
/// Returns `Err(CoreError::Validation(…))` if either supplied string is not a
/// recognised mode name.
pub fn resolve_default_mode(
    explicit: Option<&str>,
    config_default: Option<&str>,
) -> Result<SearchMode> {
    if let Some(s) = explicit {
        return SearchMode::from_str(s);
    }
    if let Some(s) = config_default {
        return SearchMode::from_str(s);
    }
    Ok(SearchMode::Lexical)
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
    /// When `true`, the hybrid merge path will populate the `score_parts` field
    /// on each [`crate::ScoredCard`]. The lexical-only path always leaves it `None`.
    pub include_score_parts: bool,
}

/// Raw search result from the store: a fetched memory plus the best matching
/// representation's bm25 score (lower = better, as SQLite returns it).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub fetched: FetchedMemory,
    pub bm25: f64,
}

/// Filter passed to `list_memories`.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub include_deleted: bool,
    pub r#type: Option<MemoryType>,
    pub limit: Option<u32>,
}

/// Sanitize a free-text query for FTS5 MATCH. Collapses to alphanumeric
/// tokens (plus `-` and `_`), then wraps each token in double quotes so
/// FTS5 treats it as a literal phrase. Without quoting, FTS5 parses tokens
/// like `soft-delete` as `soft:delete` (column reference), surfacing
/// `no such column: delete` errors on perfectly valid English queries.
/// Quoting also short-circuits other FTS5 syntax characters (`*`, `(`,
/// `OR`, `NEAR`, etc.) without us needing per-character escaping.
///
/// Returns empty string when the query has no usable tokens — callers
/// should skip the search in that case.
pub fn sanitize_fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn resolve_default_mode_precedence() {
        // explicit beats config default
        assert_eq!(
            resolve_default_mode(Some("semantic"), Some("hybrid")).unwrap(),
            SearchMode::Semantic
        );
        // config default used when no explicit
        assert_eq!(
            resolve_default_mode(None, Some("hybrid")).unwrap(),
            SearchMode::Hybrid
        );
        // Lexical fallback when both absent
        assert_eq!(
            resolve_default_mode(None, None).unwrap(),
            SearchMode::Lexical
        );
        // bad explicit → error
        assert!(matches!(
            resolve_default_mode(Some("fuzzy"), None),
            Err(CoreError::Validation(_))
        ));
        // bad config → error
        assert!(matches!(
            resolve_default_mode(None, Some("bad")),
            Err(CoreError::Validation(_))
        ));
    }

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

    #[test]
    fn sanitize_strips_fts_specials() {
        assert_eq!(sanitize_fts_query("MCP adapter!"), "\"MCP\" \"adapter\"");
        assert_eq!(
            sanitize_fts_query("  (foo) \"bar\" baz-qux "),
            "\"foo\" \"bar\" \"baz-qux\""
        );
        assert_eq!(sanitize_fts_query("***"), "");
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn sanitize_quotes_dashed_terms_so_fts5_does_not_parse_as_column_ref() {
        // Without quoting, FTS5 reads `soft-delete` as `soft:delete` and
        // raises `no such column: delete`. The quoted form is a phrase.
        assert_eq!(
            sanitize_fts_query("FTS triggers soft-delete sync"),
            "\"FTS\" \"triggers\" \"soft-delete\" \"sync\""
        );
        assert_eq!(sanitize_fts_query("auto-memorise"), "\"auto-memorise\"");
        assert_eq!(sanitize_fts_query("opt-in"), "\"opt-in\"");
    }
}
