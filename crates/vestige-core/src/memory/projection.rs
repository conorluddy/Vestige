//! Fetched-memory → cards / details projection — `FetchedMemory`, `MemoryCard`,
//! `MemoryDetail`, and the `project_card` / `project_detail` / `pick_representation`
//! functions that convert raw store rows into agent-friendly shapes.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::MemoryId;
use crate::types::{Memory, MemoryStatus, MemoryType, RepresentationDepth};

use super::bundle::{RepresentationRow, SourceRow};

/// What the store returns after fetching a memory + its joined rows.
#[derive(Debug, Clone)]
pub struct FetchedMemory {
    pub memory: Memory,
    pub representations: Vec<RepresentationRow>,
    pub sources: Vec<SourceRow>,
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

/// Project a [`FetchedMemory`] into a compact [`MemoryCard`] for list/search results.
///
/// The title is derived from the `OneLiner` representation, truncated at the
/// first word boundary that fits within 60 chars. Missing representations yield
/// empty strings rather than errors — callers that need a hard invariant should
/// assert `available_depths` on the returned card.
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

/// Project a [`FetchedMemory`] into a [`MemoryDetail`] for `vestige show` and
/// `vestige_expand`. Includes all representations and source rows in addition
/// to the compact card produced by [`project_card`].
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

// ========================================
// === PRIVATE HELPERS ===
// ========================================

fn derive_title_from_one_liner(one_liner: &str) -> String {
    // The one-liner is already short enough by construction (first sentence).
    // Re-using `derive` would re-enter the title-truncation rule, so keep it
    // direct here — same MAX and same shared `truncate_at_word` as
    // `representations::derive_title`.
    const MAX: usize = 60;
    if one_liner.chars().count() <= MAX {
        return one_liner.to_string();
    }
    crate::representations::truncate_at_word(one_liner, MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProjectId;
    use crate::memory::bundle::{build_bundle, NewMemory};
    use crate::types::MemoryType;

    fn project() -> ProjectId {
        ProjectId::from_slug("test")
    }

    #[test]
    fn project_card_populates_title_and_one_liner() {
        let bundle = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Note,
                body: "Use SQLite as canonical store.",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        let fetched = FetchedMemory {
            memory: bundle.memory,
            representations: bundle.representations,
            sources: vec![],
        };
        let card = project_card(&fetched);
        assert!(!card.title.is_empty());
        assert!(!card.one_liner.is_empty());
        assert_eq!(card.available_depths.len(), 4);
    }

    #[test]
    fn derive_title_from_one_liner_matches_representations_derive_title() {
        // Both call the same shared `truncate_at_word` — assert identical
        // output for identical input to prove the dedup is behaviour-preserving.
        let cases = [
            "Short one liner",
            "This is a very long one liner that definitely exceeds the sixty character title limit by a wide margin",
            "supercalifragilisticexpialidocioussupercalifragilisticexpialidocious",
        ];
        for case in cases {
            let from_one_liner = derive_title_from_one_liner(case);
            let from_representations = crate::representations::derive(case).title;
            assert_eq!(
                from_one_liner, from_representations,
                "derive_title_from_one_liner and representations::derive_title must agree for {case:?}"
            );
        }
    }

    #[test]
    fn project_detail_includes_representations_and_sources() {
        let bundle = build_bundle(
            &project(),
            NewMemory {
                r#type: MemoryType::Decision,
                body: "Use SQLite as canonical store.",
                importance: 0.8,
                source: None,
            },
        )
        .unwrap();
        let fetched = FetchedMemory {
            memory: bundle.memory,
            representations: bundle.representations,
            sources: vec![],
        };
        let detail = project_detail(&fetched);
        assert_eq!(detail.representations.len(), 4);
        assert!(detail.sources.is_empty());
    }
}
