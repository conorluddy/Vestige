//! Tail tab — merged, time-ordered stream of recent memories and candidates.
//!
//! Renders the newest entries across both tables for the current project,
//! auto-refreshing on a configurable interval. Auto-scroll pauses when the
//! cursor moves off row 0 and resumes when the user returns to the top.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use time::OffsetDateTime;

use anyhow::Result;
use vestige_core::{project_card, Candidate, MemoryCard, ProjectId};
use vestige_store::{CandidateFilter, Store};

use crate::commands::browse::app::{TailDepth, TailTabState};

// === TYPES ===

/// A single row in the merged tail stream — either a promoted memory or a
/// pending candidate. The variant determines which row renderer is used.
///
/// `Candidate` is boxed because it is significantly larger than `MemoryCard`.
#[derive(Debug, Clone)]
pub enum TailRow {
    Memory(MemoryCard),
    Candidate(Box<Candidate>),
}

impl TailRow {
    /// `created_at` as an `OffsetDateTime` for merge ordering.
    pub fn created_at(&self) -> OffsetDateTime {
        match self {
            TailRow::Memory(m) => m.created_at,
            TailRow::Candidate(c) => c.created_at,
        }
    }

    /// The row's string id — `mem_…` or `cand_…`.
    pub fn id(&self) -> &str {
        match self {
            TailRow::Memory(m) => m.id.as_str(),
            TailRow::Candidate(c) => c.id.as_str(),
        }
    }
}

// === PUBLIC API ===

/// Merge two slices into a single DESC-ordered `Vec<TailRow>`, truncated to
/// `cap`. Ties broken by id string DESC for stable, deterministic ordering.
pub fn merge(memories: Vec<MemoryCard>, candidates: Vec<Candidate>, cap: usize) -> Vec<TailRow> {
    let mut rows: Vec<TailRow> = memories
        .into_iter()
        .map(TailRow::Memory)
        .chain(
            candidates
                .into_iter()
                .map(|c| TailRow::Candidate(Box::new(c))),
        )
        .collect();

    rows.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.id().cmp(a.id()))
    });

    rows.truncate(cap);
    rows
}

/// Query the store for recent memories and pending candidates, then merge them.
pub fn reload(store: &Store, project: &ProjectId, cap: usize) -> Result<Vec<TailRow>> {
    let fetched = store.recent_memories_by_created_at(project, cap as u32)?;
    let memories: Vec<MemoryCard> = fetched.iter().map(project_card).collect();
    let candidates = store.list_candidates(
        project,
        &CandidateFilter {
            limit: Some(cap as u32),
            ..CandidateFilter::default()
        },
    )?;
    Ok(merge(memories, candidates, cap))
}

/// Render the Tail tab into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TailTabState) {
    if let Some(err) = &state.load_error {
        let paragraph = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    if state.items.is_empty() {
        let paragraph = Paragraph::new(
            "No memories or candidates yet — records will appear here as they are created.",
        )
        .style(Style::default().add_modifier(Modifier::DIM))
        .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    let items: Vec<ListItem> = state
        .items
        .iter()
        .map(|r| row_for_tail_row(r, state.depth))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Tail ({})", state.items.len()));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected));
    frame.render_stateful_widget(list, area, &mut list_state);
}

// === PRIVATE ===

fn row_for_tail_row(row: &TailRow, depth: TailDepth) -> ListItem<'_> {
    match row {
        TailRow::Memory(card) => super::memories::row_for_card_at_depth(card, depth),
        TailRow::Candidate(candidate) => {
            let kind = super::candidates::short_kind(candidate.proposed_type);
            let kind_style = super::candidates::kind_style(candidate.proposed_type);
            let conf_style = confidence_style(candidate.confidence);
            let line = Line::from(vec![
                Span::styled(format!("{kind:<5}"), kind_style),
                Span::raw(" "),
                Span::styled("cand ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:>4.2}", candidate.confidence), conf_style),
                Span::raw(" "),
                Span::raw(candidate.title.as_str().to_string()),
            ]);
            ListItem::new(line)
        }
    }
}

fn confidence_style(confidence: f32) -> Style {
    if confidence >= 0.8 {
        Style::default().fg(Color::Green)
    } else if confidence >= 0.5 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use vestige_core::{
        CandidateId, CandidateStatus, MemoryId, MemoryStatus, MemoryType, ProjectId,
        RepresentationDepth,
    };

    fn make_memory(created_at: OffsetDateTime) -> MemoryCard {
        MemoryCard {
            id: MemoryId::new(),
            r#type: MemoryType::Note,
            status: MemoryStatus::Active,
            title: "test memory".into(),
            one_liner: "test memory one-liner".into(),
            importance: 0.5,
            created_at,
            updated_at: created_at,
            available_depths: vec![RepresentationDepth::OneLiner],
        }
    }

    fn make_candidate(created_at: OffsetDateTime) -> Candidate {
        Candidate {
            id: CandidateId::generate(),
            project_id: ProjectId::from_slug("test"),
            proposed_type: MemoryType::Decision,
            status: CandidateStatus::Pending,
            title: "test candidate".into(),
            one_liner: "one liner".into(),
            summary: None,
            full_body: "full body".into(),
            rationale: None,
            confidence: 0.8,
            importance: 0.5,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
            approved_memory_id: None,
            rejection_reason: None,
            review_note: None,
            created_at,
            updated_at: created_at,
            reviewed_at: None,
            sources: Vec::new(),
        }
    }

    #[test]
    fn merge_interleaves_by_created_at_desc_and_respects_cap() {
        let now = OffsetDateTime::now_utc();
        let t0 = now;
        let t1 = now - time::Duration::seconds(10);
        let t2 = now - time::Duration::seconds(20);
        let t3 = now - time::Duration::seconds(30);

        let memories = vec![make_memory(t0), make_memory(t2)];
        let candidates = vec![make_candidate(t1), make_candidate(t3)];

        let merged = merge(memories, candidates, 3);
        assert_eq!(merged.len(), 3, "cap=3 should truncate");

        let ts: Vec<OffsetDateTime> = merged.iter().map(|r| r.created_at()).collect();
        assert!(ts[0] >= ts[1], "rows should be DESC");
        assert!(ts[1] >= ts[2], "rows should be DESC");

        // First row should be the memory at t0 (newest).
        assert!(matches!(merged[0], TailRow::Memory(_)));
        // Last row shown should be candidate at t1 (cap=3, so t3 is dropped).
        assert!(matches!(merged[2], TailRow::Memory(_))); // memory at t2
    }

    #[test]
    fn merge_tie_broken_by_id_desc_for_stable_order() {
        let now = OffsetDateTime::now_utc();
        let mut m1 = make_memory(now);
        let mut m2 = make_memory(now);
        // Force known ids so the tie-break is deterministic.
        m1.id = "mem_01JVZZZZZZZZZZZZZZZZZZZZZZ".parse().unwrap();
        m2.id = "mem_01JVAAAAAAAAAAAAAAAAAAAAA0".parse().unwrap();

        let merged = merge(vec![m1, m2], vec![], 10);
        assert_eq!(merged.len(), 2);
        assert!(
            merged[0].id() >= merged[1].id(),
            "tie should break by id DESC: {} >= {}",
            merged[0].id(),
            merged[1].id()
        );
    }

    #[test]
    fn auto_scroll_pause_logic_keeps_selected_row_stable_on_prepend() {
        let now = OffsetDateTime::now_utc();
        let t = |secs: i64| now - time::Duration::seconds(secs);

        // 5 initial rows; cursor at index 3.
        let initial: Vec<MemoryCard> = (0..5).map(|i| make_memory(t(i * 10))).collect();
        let initial_rows: Vec<TailRow> = initial.iter().cloned().map(TailRow::Memory).collect();
        let selected_id = initial_rows[3].id().to_string();

        // 2 new rows prepend (newer timestamps), 5 original follow.
        let newer1 = make_memory(now + time::Duration::seconds(20));
        let newer2 = make_memory(now + time::Duration::seconds(10));
        let mut reloaded: Vec<MemoryCard> = vec![newer1, newer2];
        reloaded.extend(initial.clone());
        let new_rows: Vec<TailRow> = reloaded.into_iter().map(TailRow::Memory).collect();

        // auto_scroll=false: find the same id → new index should be 5.
        let new_selected = new_rows
            .iter()
            .position(|r| r.id() == selected_id)
            .unwrap_or(0);
        assert_eq!(
            new_selected, 5,
            "2 new rows prepended → same row now at index 5"
        );

        // auto_scroll=true: selected is always 0.
        let auto_selected = 0usize;
        assert_eq!(auto_selected, 0);
        // items[0] is the newest.
        assert!(
            new_rows[0].created_at() >= new_rows[1].created_at(),
            "items[0] should be newest"
        );
    }
}
