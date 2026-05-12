//! Candidates tab — list pane + detail pane for `vestige browse`.
//!
//! Same shape as `tabs::memories`: two-pane list/detail, filter via case-
//! insensitive substring on title (candidates are low-volume, so we don't
//! lean on FTS for this), provenance sub-views (`w` why, `s` sources).
//! `t` (traces-of) is not meaningful for candidates and is gated by
//! `app.tab` in the dispatcher.

use anyhow::Result;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use vestige_core::{Candidate, MemoryType, ProjectId};
use vestige_store::{CandidateFilter, Store};

use crate::commands::browse::app::{App, DetailView};

const LIST_CAP: u32 = 500;

// === PUBLIC API ===

/// Reload the list pane from `Store::list_candidates`, then post-filter by
/// case-insensitive substring on `title` and `one_liner` when a filter is set.
pub fn reload_list(app: &mut App, store: &Store, project_id: &ProjectId) -> Result<()> {
    let filter = CandidateFilter {
        limit: Some(LIST_CAP),
        ..CandidateFilter::default()
    };
    let state = &mut app.candidates;
    state.load_error = None;
    match store.list_candidates(project_id, &filter) {
        Ok(all) => {
            let needle = state.filter_text.trim().to_lowercase();
            state.items = if needle.is_empty() {
                all
            } else {
                all.into_iter()
                    .filter(|c| {
                        c.title.to_lowercase().contains(&needle)
                            || c.one_liner.to_lowercase().contains(&needle)
                    })
                    .collect()
            };
            if state.selected >= state.items.len() {
                state.selected = state.items.len().saturating_sub(1);
            }
        }
        Err(e) => {
            state.items.clear();
            state.load_error = Some(format!("load failed: {e}"));
        }
    }
    refresh_detail(app, store)?;
    Ok(())
}

/// Re-fetch the full candidate for the selected row (including sources).
pub fn refresh_detail(app: &mut App, store: &Store) -> Result<()> {
    let state = &mut app.candidates;
    let Some(id) = state.selected_id().cloned() else {
        state.detail = None;
        return Ok(());
    };
    match store.get_candidate(&id) {
        Ok(detail) => state.detail = detail,
        Err(e) => {
            state.detail = None;
            state.load_error = Some(format!("detail load failed: {e}"));
        }
    }
    Ok(())
}

/// Load provenance for the selected candidate. Uses the candidate-specific
/// store helpers — `fetch_candidate_events` (no `memory_id` index, so reads
/// via `json_extract`) and `fetch_candidate_sources_with_ids`.
pub fn ensure_provenance(app: &mut App, store: &Store, view: DetailView) -> Result<()> {
    let state = &mut app.candidates;
    let Some(id) = state.selected_id().cloned() else {
        return Ok(());
    };
    match view {
        DetailView::Why => {
            if state.provenance.events.is_none() {
                let events = store.fetch_candidate_events(&id).unwrap_or_default();
                state.provenance.events = Some(events);
            }
        }
        DetailView::Sources => {
            if state.provenance.sources.is_none() {
                let sources = store
                    .fetch_candidate_sources_with_ids(&id, None)
                    .unwrap_or_default();
                state.provenance.sources = Some(sources);
            }
        }
        DetailView::TracesOf | DetailView::Default => {}
    }
    state.detail_view = view;
    Ok(())
}

/// Draw the Candidates tab into `area`.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(err) = &app.candidates.load_error {
        let p = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    }

    if app.candidates.items.is_empty() {
        draw_empty(frame, area, &app.candidates.filter_text);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_list(frame, chunks[0], app);
    draw_detail(frame, chunks[1], app);

    if app.candidates.filter_focused || !app.candidates.filter_text.is_empty() {
        draw_filter_prompt(frame, area, app);
    }
}

// === PRIVATE ===

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.candidates.items.iter().map(row_for_candidate).collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Candidates ({})", app.candidates.items.len()));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(app.candidates.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn row_for_candidate(cand: &Candidate) -> ListItem<'_> {
    let kind = short_kind(cand.proposed_type);
    let kind_style = kind_style(cand.proposed_type);
    let conf_style = if cand.confidence >= 0.8 {
        Style::default().fg(Color::Green)
    } else if cand.confidence >= 0.5 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red)
    };
    let line = Line::from(vec![
        Span::styled(format!("{kind:<5}"), kind_style),
        Span::raw(" "),
        Span::styled(format!("{:>4.2}", cand.confidence), conf_style),
        Span::raw(" "),
        Span::raw(cand.title.clone()),
    ]);
    ListItem::new(line)
}

fn short_kind(t: MemoryType) -> &'static str {
    match t {
        MemoryType::Decision => "dec",
        MemoryType::Note => "note",
        MemoryType::OpenQuestion => "q",
        MemoryType::Observation => "obs",
        MemoryType::Preference => "pref",
        MemoryType::ProjectSummary => "sum",
    }
}

fn kind_style(t: MemoryType) -> Style {
    match t {
        MemoryType::Decision => Style::default().fg(Color::Yellow),
        MemoryType::Note => Style::default().fg(Color::Gray),
        MemoryType::OpenQuestion => Style::default().fg(Color::Magenta),
        MemoryType::Observation => Style::default().fg(Color::Blue),
        MemoryType::Preference => Style::default().fg(Color::Green),
        MemoryType::ProjectSummary => Style::default().fg(Color::Cyan),
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let (title, breadcrumb): (&str, Option<&str>) = match app.candidates.detail_view {
        DetailView::Default => ("Detail", None),
        DetailView::Why => ("Detail · why", Some("Esc — back to detail")),
        DetailView::Sources => ("Detail · sources", Some("Esc — back to detail")),
        DetailView::TracesOf => ("Detail", None), // not exposed for candidates
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(cand) = &app.candidates.detail else {
        let p = Paragraph::new("(no selection)")
            .style(Style::default().add_modifier(Modifier::DIM))
            .alignment(Alignment::Center);
        frame.render_widget(p, inner);
        return;
    };

    let lines = match app.candidates.detail_view {
        DetailView::Default => default_lines(cand),
        DetailView::Why => why_lines(cand, app.candidates.provenance.events.as_deref()),
        DetailView::Sources => sources_lines(cand, app.candidates.provenance.sources.as_deref()),
        DetailView::TracesOf => default_lines(cand),
    };
    let mut final_lines = lines;
    if let Some(hint) = breadcrumb {
        final_lines.push(Line::from(""));
        final_lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::Gray),
        )));
    }
    let paragraph = Paragraph::new(final_lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn header_lines(cand: &Candidate) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            cand.id.as_str().to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:?}", cand.proposed_type),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("conf {:.2}", cand.confidence),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("imp {:.2}", cand.importance),
            Style::default().fg(Color::Gray),
        ),
    ]));
    lines.push(Line::from(""));
    lines
}

fn default_lines(cand: &Candidate) -> Vec<Line<'static>> {
    let mut lines = header_lines(cand);
    lines.push(Line::from(Span::styled(
        cand.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if let Some(summary) = cand.summary.as_deref().filter(|s| !s.is_empty()) {
        lines.push(Line::from(""));
        for chunk in summary.split('\n') {
            lines.push(Line::from(chunk.to_string()));
        }
    } else if !cand.one_liner.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(cand.one_liner.clone()));
    }
    if let Some(rationale) = &cand.rationale {
        if !rationale.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "rationale",
                Style::default().fg(Color::Gray),
            )));
            lines.push(Line::from(rationale.clone()));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "a approve  ·  R reject  ·  w why  ·  s sources",
        Style::default().fg(Color::Gray),
    )));
    lines
}

fn why_lines(
    cand: &Candidate,
    events: Option<&[vestige_store::ProvenanceEvent]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(cand);
    let Some(events) = events else {
        lines.push(Line::from(Span::styled(
            "loading…",
            Style::default().add_modifier(Modifier::DIM),
        )));
        return lines;
    };
    if events.is_empty() {
        lines.push(Line::from(Span::styled(
            "No journal events for this candidate.",
            Style::default().fg(Color::Yellow),
        )));
        return lines;
    }
    for evt in events {
        lines.push(Line::from(vec![
            Span::styled(evt.event_at.clone(), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(
                evt.event_type.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(evt.event_id.clone(), Style::default().fg(Color::Gray)),
        ]));
    }
    lines
}

fn sources_lines(
    cand: &Candidate,
    sources: Option<&[vestige_store::SourceReceiptRow]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(cand);
    let Some(sources) = sources else {
        lines.push(Line::from(Span::styled(
            "loading…",
            Style::default().add_modifier(Modifier::DIM),
        )));
        return lines;
    };
    if sources.is_empty() {
        lines.push(Line::from(Span::styled(
            "No source receipts attached.",
            Style::default().fg(Color::Yellow),
        )));
        return lines;
    }
    for src in sources {
        lines.push(Line::from(vec![
            Span::styled(
                src.source_type.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(src.source_id.clone(), Style::default().fg(Color::Gray)),
        ]));
        if let Some(reference) = &src.source_ref {
            lines.push(Line::from(Span::raw(format!("  ref: {reference}"))));
        }
        if let Some(content) = &src.source_content {
            lines.push(Line::from(Span::styled(
                format!("  {}", preview(content, 240)),
                Style::default().fg(Color::Gray),
            )));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn draw_empty(frame: &mut Frame, area: Rect, filter_text: &str) {
    let lines = if filter_text.trim().is_empty() {
        vec![
            Line::from(Span::styled(
                "Inbox empty.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Candidates accumulate from auto-memorise."),
            Line::from(""),
            Line::from(Span::styled(
                "Or propose one manually:",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "  vestige candidate add \"…\"",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                "  (MCP) vestige_propose_candidate",
                Style::default().fg(Color::Cyan),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("No candidates match \"{}\".", filter_text.trim()),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Press Esc to clear the filter, or keep typing."),
        ]
    };
    let h = (lines.len() as u16 + 2).min(area.height);
    let top = (area.height.saturating_sub(h)) / 2;
    let rect = Rect {
        x: area.x,
        y: area.y + top,
        width: area.width,
        height: h,
    };
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);
}

fn draw_filter_prompt(frame: &mut Frame, area: Rect, app: &App) {
    let cursor = if app.candidates.filter_focused {
        "_"
    } else {
        ""
    };
    let text = format!("/{}{}", app.candidates.filter_text, cursor);
    let style = if app.candidates.filter_focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let bar = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), bar);
}

fn preview(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.replace('\n', " ")
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::browse::app::{CandidatesTabState, Counts, Tab};
    use ratatui::{backend::TestBackend, Terminal};
    use time::OffsetDateTime;
    use vestige_core::{CandidateId, CandidateStatus, MemoryType};

    fn candidate(label: &str) -> Candidate {
        Candidate {
            id: CandidateId::generate(),
            project_id: vestige_core::ProjectId::from_slug("test"),
            proposed_type: MemoryType::Note,
            status: CandidateStatus::Pending,
            title: format!("{label} title"),
            one_liner: format!("{label} one-liner"),
            summary: Some(format!("{label} summary")),
            full_body: format!("{label} full"),
            rationale: Some(format!("{label} why")),
            confidence: 0.62,
            importance: 0.5,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
            approved_memory_id: None,
            rejection_reason: None,
            review_note: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            reviewed_at: None,
            sources: vec![],
        }
    }

    fn app_with(state: CandidatesTabState) -> App {
        let mut a = App::new(Tab::Candidates, Counts::default(), "p".into());
        a.candidates = state;
        a
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 30,
        };
        terminal.draw(|f| draw(f, area, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut s = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                s.push_str(buffer[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn empty_state_explains_inbox() {
        let app = app_with(CandidatesTabState::default());
        let out = render(&app);
        assert!(out.contains("Inbox empty"));
        assert!(out.contains("auto-memorise"));
        assert!(out.contains("vestige candidate add"));
    }

    #[test]
    fn populated_list_shows_kind_confidence_title() {
        let s = CandidatesTabState {
            items: vec![candidate("first"), candidate("second")],
            ..Default::default()
        };
        let app = app_with(s);
        let out = render(&app);
        assert!(out.contains("Candidates (2)"));
        assert!(out.contains("first title"));
        assert!(out.contains("second title"));
        assert!(out.contains("0.62"));
        assert!(out.contains("note"));
    }

    #[test]
    fn reload_against_real_store_after_approve_drops_candidate() {
        use tempfile::TempDir;
        use vestige_core::{build_candidate_bundle, NewCandidate};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = vestige_core::ProjectId::from_slug("cand-tab-test");
        store
            .ensure_project(&proj, "Cand Tab", Some("/tmp/test"), None)
            .unwrap();

        let bundle = build_candidate_bundle(NewCandidate {
            project_id: proj.clone(),
            proposed_type: MemoryType::Note,
            body: "Worth keeping?".into(),
            rationale: None,
            title_override: None,
            importance: 0.5,
            confidence: 0.7,
            source: None,
            duplicate_of_memory_id: None,
            duplicate_of_candidate_id: None,
        })
        .unwrap();
        let cand_id = bundle.id.clone();
        store.record_candidate(&bundle).unwrap();

        let mut app = app_with(CandidatesTabState::default());
        reload_list(&mut app, &store, &proj).unwrap();
        assert_eq!(app.candidates.items.len(), 1);
        assert_eq!(app.candidates.items[0].id, cand_id);

        // Approve through the engine path
        let mem_id = vestige_core::MemoryId::new();
        store.mark_candidate_approved(&cand_id, &mem_id).unwrap();

        reload_list(&mut app, &store, &proj).unwrap();
        assert_eq!(
            app.candidates.items.len(),
            0,
            "approved candidate is no longer pending"
        );
    }

    #[test]
    fn detail_pane_shows_summary_and_action_keys() {
        let cand = candidate("alpha");
        let s = CandidatesTabState {
            items: vec![cand.clone()],
            detail: Some(cand),
            ..Default::default()
        };
        let app = app_with(s);
        let out = render(&app);
        assert!(out.contains("alpha summary"));
        assert!(out.contains("a approve"));
        assert!(out.contains("R reject"));
    }
}
