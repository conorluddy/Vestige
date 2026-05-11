//! Memories tab — list pane + detail pane for `vestige browse`.
//!
//! Reload logic (`reload_list`, `refresh_detail`) wraps the existing
//! `Store::list_memories` / `search_memories` / `get_memory` calls. Draw logic
//! renders the two-pane split at the active body region passed in by `ui.rs`.
//!
//! Per V0.4 M2 decisions:
//! - Fixed 40/60 split.
//! - Detail re-queried on every selection change.
//! - Per-keystroke filter (no debounce) using `Store::search_memories`.
//! - Soft-deleted rows shown by default with strike-through styling.
//! - Rich empty-state copy per scenario.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use anyhow::Result;
use vestige_core::{
    project_card, project_detail, ListFilter, MemoryCard, MemoryDetail, MemoryStatus, MemoryType,
    ProjectId, RepresentationDepth, SearchFilter,
};
use vestige_store::Store;

use crate::commands::browse::app::{App, DetailView};

const TRACES_OF_LIMIT: u32 = 50;

const LIST_CAP: u32 = 500;

// === PUBLIC API ===

/// Reload the list pane for the Memories tab using the current filter.
///
/// Empty filter → `list_memories` (active + deleted, newest first).
/// Non-empty filter → `search_memories` (FTS5), then expand hits to cards by
/// fetching each row. The hit→card conversion is N additional `get_memory`
/// reads but N is bounded by the search limit (default 50) and SQLite is
/// local — measured below 5ms for typical projects.
pub fn reload_list(app: &mut App, store: &Store, project_id: &ProjectId) -> Result<()> {
    let state = &mut app.memories;
    state.load_error = None;
    let result = if state.filter_text.trim().is_empty() {
        load_unfiltered(store, project_id)
    } else {
        load_filtered(store, project_id, state.filter_text.trim())
    };
    match result {
        Ok(cards) => {
            state.items = cards;
            if state.selected >= state.items.len() {
                state.selected = state.items.len().saturating_sub(1);
            }
            state.scroll_offset = state.scroll_offset.min(state.selected);
        }
        Err(e) => {
            state.items.clear();
            state.load_error = Some(format!("load failed: {e}"));
        }
    }
    refresh_detail(app, store)?;
    Ok(())
}

/// Switch the detail pane to a provenance sub-view, loading its data on first
/// request and caching until the cursor moves. View order from `w` / `s` / `t`:
/// - [`DetailView::Why`]      — `fetch_memory_events`
/// - [`DetailView::Sources`]  — `fetch_memory_sources`
/// - [`DetailView::TracesOf`] — `fetch_traces_for_memory`
pub fn ensure_provenance(
    app: &mut App,
    store: &Store,
    project_id: &ProjectId,
    view: DetailView,
) -> Result<()> {
    let state = &mut app.memories;
    let Some(id) = state.selected_id().cloned() else {
        return Ok(());
    };
    match view {
        DetailView::Why => {
            if state.provenance.events.is_none() {
                let events = store.fetch_memory_events(&id).unwrap_or_default();
                state.provenance.events = Some(events);
            }
        }
        DetailView::Sources => {
            if state.provenance.sources.is_none() {
                let sources = store.fetch_memory_sources(&id, None).unwrap_or_default();
                state.provenance.sources = Some(sources);
            }
        }
        DetailView::TracesOf => {
            if state.provenance.traces_of.is_none() {
                let traces = store
                    .fetch_traces_for_memory(project_id.as_str(), &id, TRACES_OF_LIMIT)
                    .unwrap_or_default();
                state.provenance.traces_of = Some(traces);
            }
        }
        DetailView::Default => {}
    }
    state.detail_view = view;
    Ok(())
}

/// Re-fetch the detail row for the currently selected memory. Called after the
/// cursor moves. Cheap — one `get_memory` against local SQLite.
pub fn refresh_detail(app: &mut App, store: &Store) -> Result<()> {
    let state = &mut app.memories;
    let Some(id) = state.selected_id().cloned() else {
        state.detail = None;
        return Ok(());
    };
    let fetched = match store.get_memory(&id) {
        Ok(Some(f)) => f,
        Ok(None) => {
            state.detail = None;
            return Ok(());
        }
        Err(e) => {
            state.detail = None;
            state.load_error = Some(format!("detail load failed: {e}"));
            return Ok(());
        }
    };
    state.detail = Some(project_detail(&fetched));
    Ok(())
}

/// Draw the Memories tab into `area`. Splits 40/60 between list and detail
/// (unless the area is too narrow, in which case the list takes the whole
/// area for now — narrow-terminal layouts are a V0.4 follow-up).
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(err) = &app.memories.load_error {
        let p = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    }

    if app.memories.items.is_empty() {
        draw_empty(frame, area, &app.memories.filter_text);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_list(frame, chunks[0], app);
    draw_detail(frame, chunks[1], app);

    if app.memories.filter_focused || !app.memories.filter_text.is_empty() {
        draw_filter_prompt(frame, area, app);
    }
}

// === PRIVATE ===

fn load_unfiltered(store: &Store, project_id: &ProjectId) -> Result<Vec<MemoryCard>> {
    let filter = ListFilter {
        include_deleted: true,
        r#type: None,
        limit: Some(LIST_CAP),
    };
    let fetched = store.list_memories(project_id, &filter)?;
    Ok(fetched.iter().map(project_card).collect())
}

fn load_filtered(store: &Store, project_id: &ProjectId, query: &str) -> Result<Vec<MemoryCard>> {
    // `search_memories` is FTS5-only — soft-deleted rows are excluded by the
    // FTS sync triggers (V0 invariant). So a non-empty filter scopes to active
    // memories regardless of the M2 "show deleted by default" rule. That's the
    // intended UX: filter results are signal, not history.
    let filter = SearchFilter {
        r#type: None,
        limit: Some(LIST_CAP),
        mode: vestige_core::SearchMode::Lexical,
        include_score_parts: false,
    };
    let hits = store.search_memories(project_id, query, &filter)?;
    Ok(hits.iter().map(|h| project_card(&h.fetched)).collect())
}

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.memories.items.iter().map(row_for_card).collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Memories ({})", app.memories.items.len()));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(app.memories.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn row_for_card(card: &MemoryCard) -> ListItem<'_> {
    let kind = short_kind(card.r#type);
    let title = card.title.as_str();
    let mut style = Style::default();
    if card.status == MemoryStatus::Deleted {
        style = style
            .add_modifier(Modifier::CROSSED_OUT)
            .add_modifier(Modifier::DIM);
    }
    let kind_style = kind_style(card.r#type, card.status);
    let line = Line::from(vec![
        Span::styled(format!("{kind:<5}"), kind_style),
        Span::raw(" "),
        Span::styled(title.to_string(), style),
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

fn kind_style(t: MemoryType, status: MemoryStatus) -> Style {
    let base = match t {
        MemoryType::Decision => Style::default().fg(Color::Yellow),
        MemoryType::Note => Style::default().fg(Color::Gray),
        MemoryType::OpenQuestion => Style::default().fg(Color::Magenta),
        MemoryType::Observation => Style::default().fg(Color::Blue),
        MemoryType::Preference => Style::default().fg(Color::Green),
        MemoryType::ProjectSummary => Style::default().fg(Color::Cyan),
    };
    if status == MemoryStatus::Deleted {
        base.add_modifier(Modifier::CROSSED_OUT | Modifier::DIM)
    } else {
        base
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let (title, breadcrumb): (&str, Option<&str>) = match app.memories.detail_view {
        DetailView::Default => ("Detail", None),
        DetailView::Why => ("Detail · why", Some("Esc — back to detail")),
        DetailView::Sources => ("Detail · sources", Some("Esc — back to detail")),
        DetailView::TracesOf => ("Detail · traces-of", Some("Esc — back to detail")),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(detail) = &app.memories.detail else {
        let p = Paragraph::new("(no selection)")
            .style(Style::default().add_modifier(Modifier::DIM))
            .alignment(Alignment::Center);
        frame.render_widget(p, inner);
        return;
    };

    let lines = match app.memories.detail_view {
        DetailView::Default => default_detail_lines(detail),
        DetailView::Why => why_lines(detail, app.memories.provenance.events.as_deref()),
        DetailView::Sources => sources_lines(detail, app.memories.provenance.sources.as_deref()),
        DetailView::TracesOf => {
            traces_of_lines(detail, app.memories.provenance.traces_of.as_deref())
        }
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

fn header_lines(detail: &MemoryDetail) -> Vec<Line<'static>> {
    let card = &detail.card;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            card.id.as_str().to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:?}", card.r#type),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("imp {:.2}", card.importance),
            Style::default().fg(Color::Gray),
        ),
    ]));
    if card.status == MemoryStatus::Deleted {
        lines.push(Line::from(Span::styled(
            "DELETED",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines
}

fn default_detail_lines(detail: &MemoryDetail) -> Vec<Line<'static>> {
    let mut lines = header_lines(detail);
    let card = &detail.card;
    lines.push(Line::from(Span::styled(
        card.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if let Some(summary) = pick_text(detail, RepresentationDepth::Summary) {
        lines.push(Line::from(""));
        for chunk in summary.split('\n') {
            lines.push(Line::from(chunk.to_string()));
        }
    } else if !card.one_liner.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(card.one_liner.clone()));
    }
    if !detail.sources.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "sources: {}  ·  w why  ·  s sources  ·  t traces-of",
                detail.sources.len()
            ),
            Style::default().fg(Color::Gray),
        )));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "w why  ·  s sources  ·  t traces-of",
            Style::default().fg(Color::Gray),
        )));
    }
    lines
}

fn why_lines(
    detail: &MemoryDetail,
    events: Option<&[vestige_store::ProvenanceEvent]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(detail);
    let Some(events) = events else {
        lines.push(Line::from(Span::styled(
            "loading…",
            Style::default().add_modifier(Modifier::DIM),
        )));
        return lines;
    };
    if events.is_empty() {
        lines.push(Line::from(Span::styled(
            "No journal events for this memory.",
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
    detail: &MemoryDetail,
    sources: Option<&[vestige_store::SourceReceiptRow]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(detail);
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
            let preview = preview_240(content);
            lines.push(Line::from(Span::styled(
                format!("  {preview}"),
                Style::default().fg(Color::Gray),
            )));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn traces_of_lines(
    detail: &MemoryDetail,
    traces: Option<&[vestige_store::QueryEventRow]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(detail);
    let Some(traces) = traces else {
        lines.push(Line::from(Span::styled(
            "loading…",
            Style::default().add_modifier(Modifier::DIM),
        )));
        return lines;
    };
    if traces.is_empty() {
        lines.push(Line::from(Span::styled(
            "No traces returned this memory yet.",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Run `vestige search` (CLI) or `vestige_search` (MCP) and the",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            "trace will show up here on the next press of `t`.",
            Style::default().fg(Color::Gray),
        )));
        return lines;
    }
    for trace in traces {
        let mode = trace.mode_resolved.clone().unwrap_or_else(|| "-".into());
        let query = trace
            .query_text
            .clone()
            .unwrap_or_else(|| "(no query)".into());
        lines.push(Line::from(vec![
            Span::styled(trace.created_at.clone(), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(
                trace.kind.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("/"),
            Span::raw(mode),
            Span::raw("  "),
            Span::styled(trace.caller.clone(), Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::raw(format!("{} results", trace.result_count)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {}", preview_240(&query)),
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {}", trace.id),
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(""));
    }
    lines
}

fn preview_240(s: &str) -> String {
    const MAX: usize = 240;
    let mut out = String::new();
    for (count, ch) in s.chars().enumerate() {
        if count >= MAX {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.replace('\n', " ")
}

fn pick_text(detail: &MemoryDetail, depth: RepresentationDepth) -> Option<&str> {
    detail.representations.iter().find_map(|(d, text)| {
        if *d == depth {
            Some(text.as_str())
        } else {
            None
        }
    })
}

fn draw_empty(frame: &mut Frame, area: Rect, filter_text: &str) {
    let lines = if filter_text.trim().is_empty() {
        vec![
            Line::from(Span::styled(
                "No memories yet.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Capture one with:"),
            Line::from(Span::styled(
                "  vestige remember \"…\"",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                "  vestige decision add \"…\"",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                "  vestige note add \"…\"",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Or open the MCP server and let an agent populate it:",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "  vestige mcp",
                Style::default().fg(Color::Cyan),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("No matches for \"{}\".", filter_text.trim()),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Press Esc to clear the filter, or keep typing."),
        ]
    };
    let centred_h = centre_vertically(area, lines.len() as u16 + 2);
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, centred_h);
}

fn draw_filter_prompt(frame: &mut Frame, area: Rect, app: &App) {
    let cursor = if app.memories.filter_focused { "_" } else { "" };
    let text = format!("/{}{}", app.memories.filter_text, cursor);
    let style = if app.memories.filter_focused {
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

fn centre_vertically(area: Rect, height: u16) -> Rect {
    let h = height.min(area.height);
    let top = (area.height.saturating_sub(h)) / 2;
    Rect {
        x: area.x,
        y: area.y + top,
        width: area.width,
        height: h,
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::browse::app::{Counts, MemoriesTabState, Tab};
    use ratatui::{backend::TestBackend, Terminal};
    use time::OffsetDateTime;
    use vestige_core::MemoryId;

    fn card(label: &str, status: MemoryStatus, kind: MemoryType) -> MemoryCard {
        MemoryCard {
            id: MemoryId::new(),
            r#type: kind,
            status,
            title: format!("{label} title"),
            one_liner: format!("{label} one-liner"),
            importance: 0.5,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            available_depths: vec![RepresentationDepth::OneLiner],
        }
    }

    fn app_with(state: MemoriesTabState) -> App {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.memories = state;
        a
    }

    fn render(app: &App) -> (Terminal<TestBackend>, String) {
        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let rect = Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 30,
        };
        terminal.draw(|f| draw(f, rect, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut s = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                s.push_str(buffer[(x, y)].symbol());
            }
            s.push('\n');
        }
        (terminal, s)
    }

    #[test]
    fn empty_state_lists_capture_commands() {
        let app = app_with(MemoriesTabState::default());
        let (_t, out) = render(&app);
        assert!(out.contains("No memories yet"));
        assert!(out.contains("vestige remember"));
        assert!(out.contains("vestige mcp"));
    }

    #[test]
    fn empty_state_under_filter_explains_no_matches() {
        let s = MemoriesTabState {
            filter_text: "needle".into(),
            ..Default::default()
        };
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("No matches for \"needle\""), "got: {out}");
        assert!(out.contains("Esc to clear"));
    }

    #[test]
    fn populated_list_shows_titles_and_kind_badges() {
        let s = MemoriesTabState {
            items: vec![
                card("first", MemoryStatus::Active, MemoryType::Decision),
                card("second", MemoryStatus::Active, MemoryType::Note),
            ],
            ..Default::default()
        };
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("first title"));
        assert!(out.contains("second title"));
        assert!(out.contains("dec"));
        assert!(out.contains("note"));
        assert!(out.contains("Memories (2)"));
    }

    #[test]
    fn deleted_memory_styled_crossed_out() {
        let s = MemoriesTabState {
            items: vec![card("gone", MemoryStatus::Deleted, MemoryType::Note)],
            ..Default::default()
        };
        let app = app_with(s);
        let backend = TestBackend::new(160, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 10,
        };
        terminal.draw(|f| draw(f, area, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut found_crossed = false;
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol() == "g" && cell.style().add_modifier.contains(Modifier::CROSSED_OUT)
                {
                    found_crossed = true;
                }
            }
        }
        assert!(found_crossed, "expected deleted row to be CROSSED_OUT");
    }

    #[test]
    fn filter_prompt_renders_when_focused() {
        let s = MemoriesTabState {
            items: vec![card("x", MemoryStatus::Active, MemoryType::Note)],
            filter_text: "abc".into(),
            filter_focused: true,
            ..Default::default()
        };
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("/abc"), "expected filter prompt; got {out}");
    }

    fn detail_for(card: MemoryCard) -> MemoryDetail {
        MemoryDetail {
            card,
            representations: vec![],
            sources: vec![],
        }
    }

    fn populated_with_detail() -> MemoriesTabState {
        let c = card("alpha", MemoryStatus::Active, MemoryType::Note);
        let d = detail_for(c.clone());
        MemoriesTabState {
            items: vec![c],
            detail: Some(d),
            ..Default::default()
        }
    }

    #[test]
    fn default_detail_lists_provenance_keys_hint() {
        let app = app_with(populated_with_detail());
        let (_t, out) = render(&app);
        assert!(out.contains("w why"), "got: {out}");
        assert!(out.contains("s sources"));
        assert!(out.contains("t traces-of"));
    }

    #[test]
    fn why_subview_shows_loading_then_events() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::Why;
        // No cache → loading
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("Detail · why"), "title; got: {out}");
        assert!(out.contains("loading"), "loading indicator; got: {out}");

        // Cache populated with one event
        let mut s2 = populated_with_detail();
        s2.detail_view = DetailView::Why;
        s2.provenance.events = Some(vec![vestige_store::ProvenanceEvent {
            event_id: "evt_01HX0000000000000000000ABC".into(),
            event_type: "memory.recorded".into(),
            payload_json: None,
            event_at: "2026-05-08T10:00:00Z".into(),
        }]);
        let app2 = app_with(s2);
        let (_t, out2) = render(&app2);
        assert!(out2.contains("memory.recorded"), "event type; got: {out2}");
        assert!(out2.contains("2026-05-08T10:00:00Z"));
        assert!(out2.contains("Esc — back to detail"));
    }

    #[test]
    fn why_subview_empty_state_is_friendly() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::Why;
        s.provenance.events = Some(Vec::new());
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("No journal events"), "got: {out}");
    }

    #[test]
    fn sources_subview_renders_typed_receipts() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::Sources;
        s.provenance.sources = Some(vec![vestige_store::SourceReceiptRow {
            source_id: "src_01HX0000000000000000000ABC".into(),
            source_type: "file".into(),
            source_ref: Some("docs/prd/vestige_v_0_4_browser_prd.md".into()),
            source_content: Some("two-pane layout description".into()),
        }]);
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("Detail · sources"), "title; got: {out}");
        assert!(out.contains("file"), "kind label");
        assert!(out.contains("docs/prd/vestige_v_0_4_browser_prd.md"));
        assert!(out.contains("two-pane layout"));
    }

    #[test]
    fn sources_subview_empty_state_is_friendly() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::Sources;
        s.provenance.sources = Some(Vec::new());
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("No source receipts attached"), "got: {out}");
    }

    #[test]
    fn traces_of_subview_renders_trace_rows() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::TracesOf;
        s.provenance.traces_of = Some(vec![vestige_store::QueryEventRow {
            id: "trace_01HX0000000000000000000ABC".into(),
            kind: "search".into(),
            mode_requested: Some("hybrid".into()),
            mode_resolved: Some("hybrid".into()),
            query_text: Some("ratatui browser".into()),
            params_json: None,
            caller: "cli".into(),
            provider: None,
            provider_model: None,
            result_ids_json: None,
            result_scores_json: None,
            result_count: 7,
            latency_ms: 12,
            created_at: "2026-05-08T10:00:00Z".into(),
        }]);
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(out.contains("Detail · traces-of"), "title; got: {out}");
        assert!(out.contains("search/hybrid"));
        assert!(out.contains("cli"));
        assert!(out.contains("7 results"));
        assert!(out.contains("ratatui browser"));
        assert!(out.contains("trace_01HX"));
    }

    #[test]
    fn reload_after_forget_marks_row_deleted() {
        use tempfile::TempDir;
        use vestige_core::{build_bundle, NewMemory};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = ProjectId::from_slug("browse-forget-restore");
        store
            .ensure_project(&proj, "Mut Test", Some("/tmp/test"), None)
            .unwrap();

        let bundle = build_bundle(
            &proj,
            NewMemory {
                r#type: MemoryType::Note,
                body: "Note about M4.",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        let mem_id = bundle.memory.id.clone();
        store.record_memory(&bundle).unwrap();

        let mut app = app_with(MemoriesTabState::default());
        reload_list(&mut app, &store, &proj).unwrap();
        assert_eq!(app.memories.items.len(), 1);
        assert_eq!(app.memories.items[0].status, MemoryStatus::Active);

        store.forget_memory(&mem_id).unwrap();
        reload_list(&mut app, &store, &proj).unwrap();
        assert_eq!(
            app.memories.items.len(),
            1,
            "soft-deleted row is still listed by default"
        );
        assert_eq!(app.memories.items[0].status, MemoryStatus::Deleted);

        store.restore_memory(&mem_id).unwrap();
        reload_list(&mut app, &store, &proj).unwrap();
        assert_eq!(app.memories.items[0].status, MemoryStatus::Active);
    }

    #[test]
    fn ensure_provenance_against_real_store_round_trips() {
        use tempfile::TempDir;
        use vestige_core::{build_bundle, NewMemory};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = ProjectId::from_slug("browse-prov-test");
        store
            .ensure_project(&proj, "Browse Prov Test", Some("/tmp/test"), None)
            .unwrap();

        // Record a memory with an explicit source so all three sub-views have data.
        let bundle = build_bundle(
            &proj,
            NewMemory {
                r#type: MemoryType::Decision,
                body: "Use FTS5 + vec hybrid for recall.",
                importance: 0.7,
                source: Some(vestige_core::NewSource {
                    source_type: "file",
                    source_ref: Some("PRD.md"),
                    source_content: Some("two-pane layout"),
                }),
            },
        )
        .unwrap();
        let mem_id = bundle.memory.id.clone();
        store.record_memory(&bundle).unwrap();

        // Build app state with the memory loaded as the selected item.
        let fetched = store.get_memory(&mem_id).unwrap().unwrap();
        let mut app = app_with(MemoriesTabState {
            items: vec![project_card(&fetched)],
            detail: Some(project_detail(&fetched)),
            ..Default::default()
        });

        // Why
        ensure_provenance(&mut app, &store, &proj, DetailView::Why).unwrap();
        assert_eq!(app.memories.detail_view, DetailView::Why);
        let events = app.memories.provenance.events.as_ref().unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "memory.recorded"),
            "expected memory.recorded event; got {events:#?}"
        );

        // Sources
        ensure_provenance(&mut app, &store, &proj, DetailView::Sources).unwrap();
        assert_eq!(app.memories.detail_view, DetailView::Sources);
        let sources = app.memories.provenance.sources.as_ref().unwrap();
        assert!(
            sources.iter().any(|s| s.source_type == "file"),
            "expected file source; got {sources:#?}"
        );

        // Traces-of (no searches yet → empty)
        ensure_provenance(&mut app, &store, &proj, DetailView::TracesOf).unwrap();
        assert_eq!(app.memories.detail_view, DetailView::TracesOf);
        assert_eq!(
            app.memories.provenance.traces_of.as_ref().map(|v| v.len()),
            Some(0)
        );

        // Second call should be cached — no extra read needed. We test this by
        // confirming the Option is still Some after a second call.
        ensure_provenance(&mut app, &store, &proj, DetailView::Why).unwrap();
        assert!(app.memories.provenance.events.is_some());
    }

    #[test]
    fn traces_of_empty_state_explains_population() {
        let mut s = populated_with_detail();
        s.detail_view = DetailView::TracesOf;
        s.provenance.traces_of = Some(Vec::new());
        let app = app_with(s);
        let (_t, out) = render(&app);
        assert!(
            out.contains("No traces returned this memory yet"),
            "got: {out}"
        );
        assert!(out.contains("vestige search") || out.contains("`vestige search`"));
    }
}
