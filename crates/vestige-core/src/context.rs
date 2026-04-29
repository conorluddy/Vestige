//! Project context pack assembly (PRD §15).
//!
//! Pure: takes already-fetched memories grouped by section, returns a
//! token-budgeted text pack and a structured form for `--json` / MCP.
//! All store queries happen in the caller.

use serde::{Deserialize, Serialize};

use crate::memory::{project_card, FetchedMemory, MemoryCard};

/// Rough heuristic for char-to-token ratio. We're not tokenising properly
/// in V0 — a 4 chars-per-token estimate is good enough to keep the pack
/// inside an agent's budget. MCP / agents needing precise budgeting can
/// re-render with a stricter cap.
pub const APPROX_CHARS_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSections {
    pub project_name: String,
    pub summary: Option<MemoryCard>,
    pub decisions: Vec<MemoryCard>,
    pub open_questions: Vec<MemoryCard>,
    pub recent: Vec<MemoryCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub sections: ContextSections,
    pub text: String,
    pub approx_token_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ContextSources {
    pub project_name: String,
    pub summary: Option<FetchedMemory>,
    pub decisions: Vec<FetchedMemory>,
    pub open_questions: Vec<FetchedMemory>,
    pub recent: Vec<FetchedMemory>,
}

#[derive(Debug, Clone, Copy)]
pub struct ContextOptions {
    pub budget_tokens: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            budget_tokens: 1200,
        }
    }
}

/// Assemble a context pack from already-fetched memories. Sections are
/// emitted in fixed order: summary → decisions → open questions → recent.
/// As the running text approaches `budget_tokens * APPROX_CHARS_PER_TOKEN`,
/// further entries are skipped and `truncated` is set.
pub fn build_pack(sources: ContextSources, opts: ContextOptions) -> ContextPack {
    let budget_chars = opts.budget_tokens.saturating_mul(APPROX_CHARS_PER_TOKEN);
    let mut text = String::new();
    let mut truncated = false;

    text.push_str(&format!("Project: {}\n", sources.project_name));

    let summary_card = sources.summary.as_ref().map(project_card);
    if let Some(summary) = &sources.summary {
        let body = summary
            .representations
            .iter()
            .find(|r| r.depth == crate::types::RepresentationDepth::Summary)
            .map(|r| r.content.as_str())
            .unwrap_or("");
        text.push_str("\nSummary:\n");
        text.push_str(body);
        text.push('\n');
    }

    let mut decision_cards = Vec::new();
    if !sources.decisions.is_empty() {
        text.push_str("\nCurrent decisions:\n");
        for fetched in &sources.decisions {
            let card = project_card(fetched);
            let line = format!("- {}\n", card.one_liner.trim());
            if would_overflow(&text, &line, budget_chars) {
                truncated = true;
                break;
            }
            text.push_str(&line);
            decision_cards.push(card);
        }
    }

    let mut question_cards = Vec::new();
    if !sources.open_questions.is_empty() {
        text.push_str("\nOpen questions:\n");
        for fetched in &sources.open_questions {
            let card = project_card(fetched);
            let line = format!("- {}\n", card.one_liner.trim());
            if would_overflow(&text, &line, budget_chars) {
                truncated = true;
                break;
            }
            text.push_str(&line);
            question_cards.push(card);
        }
    }

    let mut recent_cards = Vec::new();
    if !sources.recent.is_empty() {
        text.push_str("\nRecent important memories:\n");
        for fetched in &sources.recent {
            let card = project_card(fetched);
            let line = format!("- [{}] {}\n", card.r#type, card.one_liner.trim());
            if would_overflow(&text, &line, budget_chars) {
                truncated = true;
                break;
            }
            text.push_str(&line);
            recent_cards.push(card);
        }
    }

    let approx_token_count = text.len() / APPROX_CHARS_PER_TOKEN;
    ContextPack {
        sections: ContextSections {
            project_name: sources.project_name,
            summary: summary_card,
            decisions: decision_cards,
            open_questions: question_cards,
            recent: recent_cards,
        },
        text,
        approx_token_count,
        truncated,
    }
}

fn would_overflow(current: &str, addition: &str, budget_chars: usize) -> bool {
    if budget_chars == 0 {
        return false;
    }
    current.len() + addition.len() > budget_chars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{build_bundle, NewMemory};
    use crate::types::MemoryType;
    use crate::ProjectId;

    fn fetched(ty: MemoryType, body: &str, importance: f64) -> FetchedMemory {
        let bundle = build_bundle(
            &ProjectId::from_slug("p"),
            NewMemory {
                r#type: ty,
                body,
                importance,
                source: None,
            },
        )
        .unwrap();
        FetchedMemory {
            memory: bundle.memory,
            representations: bundle.representations,
            sources: vec![],
        }
    }

    #[test]
    fn assembles_full_pack() {
        let sources = ContextSources {
            project_name: "Vestige".into(),
            summary: Some(fetched(
                MemoryType::ProjectSummary,
                "Vestige is a memory layer.",
                0.9,
            )),
            decisions: vec![fetched(
                MemoryType::Decision,
                "Use SQLite as the canonical store.",
                0.8,
            )],
            open_questions: vec![fetched(
                MemoryType::OpenQuestion,
                "Embeddings in V0.1 or V0?",
                0.5,
            )],
            recent: vec![fetched(MemoryType::Note, "MCP is a thin adapter.", 0.6)],
        };
        let pack = build_pack(sources, ContextOptions::default());
        assert!(pack.text.starts_with("Project: Vestige"));
        assert!(pack.text.contains("Summary:"));
        assert!(pack.text.contains("Current decisions:"));
        assert!(pack.text.contains("Use SQLite"));
        assert!(pack.text.contains("Open questions:"));
        assert!(pack.text.contains("Embeddings"));
        assert!(pack.text.contains("Recent important memories:"));
        assert!(!pack.truncated);
        assert_eq!(pack.sections.decisions.len(), 1);
    }

    #[test]
    fn budget_truncates_long_lists() {
        let many: Vec<FetchedMemory> = (0..50)
            .map(|i| {
                fetched(
                    MemoryType::Decision,
                    &format!("Decision number {i} which is reasonably wordy."),
                    0.5,
                )
            })
            .collect();
        let sources = ContextSources {
            project_name: "P".into(),
            summary: None,
            decisions: many,
            open_questions: vec![],
            recent: vec![],
        };
        let pack = build_pack(
            sources,
            ContextOptions {
                budget_tokens: 50, // ~200 chars
            },
        );
        assert!(pack.truncated);
        assert!(pack.text.len() <= 50 * APPROX_CHARS_PER_TOKEN + 100);
        assert!(pack.sections.decisions.len() < 50);
    }
}
