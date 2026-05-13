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
use vestige_config::traces_config_for;
use vestige_core::{
    project_card, FetchedMemory, ListFilter, MemoryCard, MemoryStatus, MemoryType, ProjectId,
    RepresentationDepth, SearchFilter, SearchMode,
};
use vestige_embed::EmbeddingProvider;
use vestige_engine::search::{search_hybrid, search_semantic};
use vestige_engine::Caller;
use vestige_store::Store;

use crate::commands::browse::app::{App, DetailView};

const TRACES_OF_LIMIT: u32 = 50;

const LIST_CAP: u32 = 500;

// === PUBLIC API ===

/// Reload the list pane for the Memories tab using the current filter.
///
/// Empty filter → `list_memories` (active + deleted, newest first).
/// Non-empty filter → dispatches to `search_lexical / search_semantic /
/// search_hybrid` based on `app.search_mode`. `provider` must be `Some` when
/// mode is Semantic or Hybrid; when `None`, the mode falls back to Lexical.
pub fn reload_list(
    app: &mut App,
    store: &Store,
    project_id: &ProjectId,
    provider: Option<&dyn EmbeddingProvider>,
) -> Result<()> {
    let filter_text = app.memories.filter_text.trim().to_string();
    let kind = app.memories_kind_filter;
    let status = app.memories_status_filter;
    let mode = if provider.is_none() {
        SearchMode::Lexical
    } else {
        app.search_mode
    };
    let result = if filter_text.is_empty() {
        load_unfiltered(store, project_id, kind, status)
    } else {
        load_filtered(
            store,
            project_id,
            &filter_text,
            kind,
            status,
            mode,
            provider,
        )
    };
    let state = &mut app.memories;
    state.load_error = None;
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
                    .fetch_traces_for_memory(project_id, &id, TRACES_OF_LIMIT)
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
    state.detail = Some(fetched);
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

fn load_unfiltered(
    store: &Store,
    project_id: &ProjectId,
    kind: Option<MemoryType>,
    status: Option<MemoryStatus>,
) -> Result<Vec<MemoryCard>> {
    // For unfiltered: include_deleted is true unless an explicit Active filter
    // is set. If Deleted-only is set, we still pull both then post-filter.
    let include_deleted = !matches!(status, Some(MemoryStatus::Active));
    let filter = ListFilter {
        include_deleted,
        r#type: kind,
        limit: Some(LIST_CAP),
    };
    let fetched = store.list_memories(project_id, &filter)?;
    let mut cards: Vec<MemoryCard> = fetched.iter().map(project_card).collect();
    if matches!(status, Some(MemoryStatus::Deleted)) {
        cards.retain(|c| c.status == MemoryStatus::Deleted);
    }
    Ok(cards)
}

fn load_filtered(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    kind: Option<MemoryType>,
    status: Option<MemoryStatus>,
    mode: SearchMode,
    provider: Option<&dyn EmbeddingProvider>,
) -> Result<Vec<MemoryCard>> {
    // Search excludes soft-deleted rows (FTS sync trigger invariant). So a
    // non-empty filter always scopes to active memories.
    let mut cards = match mode {
        SearchMode::Lexical => {
            let filter = SearchFilter {
                r#type: kind,
                limit: Some(LIST_CAP),
                mode: SearchMode::Lexical,
                include_score_parts: false,
            };
            let hits = store.search_memories(project_id, query, &filter)?;
            hits.iter()
                .map(|h| project_card(&h.fetched))
                .collect::<Vec<_>>()
        }
        SearchMode::Semantic => {
            if let Some(p) = provider {
                let traces_cfg = traces_config_for(None);
                let outcome = search_semantic(
                    store,
                    project_id,
                    query,
                    kind,
                    LIST_CAP,
                    p,
                    Caller::Cli,
                    &traces_cfg,
                )?;
                outcome.scored.iter().map(|s| s.card.clone()).collect()
            } else {
                // Provider unavailable: fall through to lexical silently.
                let filter = SearchFilter {
                    r#type: kind,
                    limit: Some(LIST_CAP),
                    mode: SearchMode::Lexical,
                    include_score_parts: false,
                };
                let hits = store.search_memories(project_id, query, &filter)?;
                hits.iter()
                    .map(|h| project_card(&h.fetched))
                    .collect::<Vec<_>>()
            }
        }
        SearchMode::Hybrid => {
            if let Some(p) = provider {
                let traces_cfg = traces_config_for(None);
                let outcome = search_hybrid(
                    store,
                    project_id,
                    query,
                    kind,
                    LIST_CAP,
                    p,
                    Caller::Cli,
                    &traces_cfg,
                )?;
                outcome.scored.iter().map(|s| s.card.clone()).collect()
            } else {
                let filter = SearchFilter {
                    r#type: kind,
                    limit: Some(LIST_CAP),
                    mode: SearchMode::Lexical,
                    include_score_parts: false,
                };
                let hits = store.search_memories(project_id, query, &filter)?;
                hits.iter()
                    .map(|h| project_card(&h.fetched))
                    .collect::<Vec<_>>()
            }
        }
    };
    if matches!(status, Some(MemoryStatus::Deleted)) {
        // FTS/vector search excludes deleted, so this will always be empty —
        // let the empty-state render naturally.
        cards.clear();
    }
    Ok(cards)
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

fn header_lines(fetched: &FetchedMemory) -> Vec<Line<'static>> {
    let m = &fetched.memory;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            m.id.as_str().to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("{:?}", m.r#type), Style::default().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(
            format!("imp {:.2}", m.importance),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("conf {:.2}", m.confidence),
            Style::default().fg(Color::Gray),
        ),
    ]));
    if m.status == MemoryStatus::Deleted {
        lines.push(Line::from(Span::styled(
            "DELETED",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines
}

fn default_detail_lines(fetched: &FetchedMemory) -> Vec<Line<'static>> {
    let mut lines = header_lines(fetched);
    let m = &fetched.memory;
    let card = project_card(fetched);

    // Title (derived from one_liner).
    lines.push(Line::from(Span::styled(
        card.title.clone(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Timestamps — absolute UTC + relative.
    lines.push(timestamp_line("created", m.created_at));
    lines.push(timestamp_line("updated", m.updated_at));
    if let Some(deleted_at) = m.deleted_at {
        lines.push(timestamp_line("deleted", deleted_at));
    }

    // Every available representation, in depth order. The OneLiner shows
    // first because it's the canonical title source; Summary/Compressed/Full
    // follow only when they exist and differ.
    let mut last: Option<&str> = None;
    for depth in [
        RepresentationDepth::OneLiner,
        RepresentationDepth::Summary,
        RepresentationDepth::Compressed,
        RepresentationDepth::Full,
    ] {
        let Some(text) = pick_text(fetched, depth) else {
            continue;
        };
        if Some(text) == last {
            // Skip identical reps (V0: Compressed is often identical to
            // Summary before the LLM-compression pass lands).
            continue;
        }
        last = Some(text);
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            depth_label(depth),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for chunk in text.split('\n') {
            lines.push(Line::from(chunk.to_string()));
        }
    }

    // Sources — inline full list rather than a count. Matches PRD §5.2
    // progressive disclosure: the detail pane is the expanded view.
    if !fetched.sources.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("sources ({})", fetched.sources.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for src in &fetched.sources {
            let mut spans = vec![Span::styled(
                src.source_type.clone(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(reference) = &src.source_ref {
                spans.push(Span::raw("  "));
                spans.push(Span::raw(reference.clone()));
            }
            if src.truncated {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    "(truncated)",
                    Style::default().fg(Color::Yellow),
                ));
            }
            lines.push(Line::from(spans));
            if let Some(content) = &src.source_content {
                lines.push(Line::from(Span::styled(
                    format!("  {}", preview_240(content)),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "w why  ·  s sources  ·  t traces-of  ·  f forget  ·  r restore",
        Style::default().fg(Color::Gray),
    )));
    lines
}

fn depth_label(depth: RepresentationDepth) -> &'static str {
    match depth {
        RepresentationDepth::OneLiner => "one-liner",
        RepresentationDepth::Summary => "summary",
        RepresentationDepth::Compressed => "compressed",
        RepresentationDepth::Full => "full",
    }
}

/// Format a timestamp row: `created   2026-05-12 14:32:11 UTC   (2 days ago)`.
/// Relative span is approximate — we floor to the largest unit that fits.
fn timestamp_line(label: &str, ts: time::OffsetDateTime) -> Line<'static> {
    let abs = format_ts(ts);
    let rel = relative_span(ts);
    Line::from(vec![
        Span::styled(format!("{label:<9}"), Style::default().fg(Color::Gray)),
        Span::raw(abs),
        Span::raw("  "),
        Span::styled(format!("({rel})"), Style::default().fg(Color::Gray)),
    ])
}

fn format_ts(ts: time::OffsetDateTime) -> String {
    // Compact UTC. Avoids pulling in a heavy format builder.
    let utc = ts.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
    )
}

fn relative_span(ts: time::OffsetDateTime) -> String {
    let now = time::OffsetDateTime::now_utc();
    let delta = now - ts;
    let secs = delta.whole_seconds();
    let abs = secs.unsigned_abs();
    let suffix = if secs >= 0 { "ago" } else { "from now" };
    let (value, unit) = match abs {
        0..=59 => (abs, "s"),
        60..=3_599 => (abs / 60, "m"),
        3_600..=86_399 => (abs / 3_600, "h"),
        86_400..=604_799 => (abs / 86_400, "d"),
        604_800..=2_591_999 => (abs / 604_800, "w"),
        2_592_000..=31_535_999 => (abs / 2_592_000, "mo"),
        _ => (abs / 31_536_000, "y"),
    };
    format!("{value}{unit} {suffix}")
}

fn why_lines(
    fetched: &FetchedMemory,
    events: Option<&[vestige_store::ProvenanceEvent]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(fetched);
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
    fetched: &FetchedMemory,
    sources: Option<&[vestige_store::SourceReceiptRow]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(fetched);
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
    fetched: &FetchedMemory,
    traces: Option<&[vestige_store::QueryEventRow]>,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(fetched);
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

fn pick_text(fetched: &FetchedMemory, depth: RepresentationDepth) -> Option<&str> {
    fetched
        .representations
        .iter()
        .find_map(|r| (r.depth == depth).then_some(r.content.as_str()))
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

    fn fetched_from_card(card: &MemoryCard) -> FetchedMemory {
        FetchedMemory {
            memory: vestige_core::Memory {
                id: card.id.clone(),
                project_id: vestige_core::ProjectId::from_slug("test"),
                r#type: card.r#type,
                status: card.status,
                confidence: 1.0,
                importance: card.importance,
                created_at: card.created_at,
                updated_at: card.updated_at,
                deleted_at: if card.status == MemoryStatus::Deleted {
                    Some(card.updated_at)
                } else {
                    None
                },
            },
            representations: vec![],
            sources: vec![],
        }
    }

    fn populated_with_detail() -> MemoriesTabState {
        let c = card("alpha", MemoryStatus::Active, MemoryType::Note);
        let d = fetched_from_card(&c);
        MemoriesTabState {
            items: vec![c],
            detail: Some(d),
            ..Default::default()
        }
    }

    #[test]
    fn default_detail_shows_every_property() {
        use vestige_core::{Memory, ProjectId, SourceRow};
        let id = vestige_core::MemoryId::new();
        let created = time::OffsetDateTime::now_utc() - time::Duration::days(3);
        let updated = time::OffsetDateTime::now_utc() - time::Duration::hours(2);
        let fetched = FetchedMemory {
            memory: Memory {
                id: id.clone(),
                project_id: ProjectId::from_slug("test"),
                r#type: MemoryType::Decision,
                status: MemoryStatus::Active,
                confidence: 0.87,
                importance: 0.64,
                created_at: created,
                updated_at: updated,
                deleted_at: None,
            },
            representations: vec![
                vestige_core::RepresentationRow {
                    memory_id: id.clone(),
                    depth: RepresentationDepth::OneLiner,
                    content: "Use FTS5 + vec for hybrid recall.".into(),
                    content_hash: "abc".into(),
                },
                vestige_core::RepresentationRow {
                    memory_id: id.clone(),
                    depth: RepresentationDepth::Summary,
                    content: "Hybrid recall blends lexical FTS5 with semantic vectors.".into(),
                    content_hash: "def".into(),
                },
                vestige_core::RepresentationRow {
                    memory_id: id.clone(),
                    depth: RepresentationDepth::Full,
                    content: "Full body explaining the choice in detail.".into(),
                    content_hash: "ghi".into(),
                },
            ],
            sources: vec![SourceRow {
                memory_id: id.clone(),
                source_type: "file".into(),
                source_ref: Some("docs/prd/vestige_v_0_4_browser_prd.md".into()),
                source_content: Some("two-pane layout discussion".into()),
                truncated: false,
            }],
        };
        let card = vestige_core::project_card(&fetched);
        let s = MemoriesTabState {
            items: vec![card],
            detail: Some(fetched),
            ..Default::default()
        };
        let app = app_with(s);
        let (_t, out) = render(&app);
        // Header
        assert!(out.contains(id.as_str()), "id; got: {out}");
        assert!(out.contains("Decision"));
        assert!(out.contains("imp 0.64"));
        assert!(out.contains("conf 0.87"));
        // Timestamps — absolute + relative
        assert!(out.contains("UTC"), "absolute ts; got: {out}");
        assert!(
            out.contains("ago") || out.contains("from now"),
            "relative ts; got: {out}"
        );
        // All three representation headers
        assert!(out.contains("one-liner"), "got: {out}");
        assert!(out.contains("summary"));
        assert!(out.contains("full"));
        // Representation bodies
        assert!(out.contains("Use FTS5 + vec for hybrid recall"));
        assert!(out.contains("Hybrid recall blends"));
        assert!(out.contains("Full body explaining"));
        // Full source content shown inline, not just count
        assert!(out.contains("sources (1)"));
        assert!(out.contains("file"));
        assert!(out.contains("docs/prd/vestige_v_0_4_browser_prd.md"));
        assert!(out.contains("two-pane layout discussion"));
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
        reload_list(&mut app, &store, &proj, None).unwrap();
        assert_eq!(app.memories.items.len(), 1);
        assert_eq!(app.memories.items[0].status, MemoryStatus::Active);

        store.forget_memory(&mem_id).unwrap();
        reload_list(&mut app, &store, &proj, None).unwrap();
        assert_eq!(
            app.memories.items.len(),
            1,
            "soft-deleted row is still listed by default"
        );
        assert_eq!(app.memories.items[0].status, MemoryStatus::Deleted);

        store.restore_memory(&mem_id).unwrap();
        reload_list(&mut app, &store, &proj, None).unwrap();
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
            detail: Some(fetched),
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

    /// `reload_list` with `SearchMode::Lexical` and no provider works identically
    /// to the hardcoded path that was removed. Regression guard.
    #[test]
    fn reload_list_lexical_with_no_provider_returns_results() {
        use tempfile::TempDir;
        use vestige_core::{build_bundle, NewMemory};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = ProjectId::from_slug("browse-lexical-no-prov");
        store
            .ensure_project(&proj, "Lex Test", Some("/tmp/test"), None)
            .unwrap();

        let bundle = build_bundle(
            &proj,
            NewMemory {
                r#type: MemoryType::Note,
                body: "FTS5 filter test note for lexical search.",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        store.record_memory(&bundle).unwrap();

        let mut app = app_with(MemoriesTabState::default());
        // Non-empty filter with no provider: must fall back to lexical.
        app.memories.filter_text = "FTS5".into();
        reload_list(&mut app, &store, &proj, None).unwrap();
        assert!(
            !app.memories.items.is_empty(),
            "lexical search should return results"
        );
    }

    /// `reload_list` with `SearchMode::Semantic` and the fake provider finds
    /// memories (fake vectors are deterministic and all cosine-similar).
    #[test]
    fn reload_list_semantic_with_fake_provider_returns_results() {
        use tempfile::TempDir;
        use vestige_core::{build_bundle, NewMemory};
        use vestige_embed::{build_provider, EmbeddingsConfig};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = ProjectId::from_slug("browse-semantic-fake");
        store
            .ensure_project(&proj, "Sem Test", Some("/tmp/test"), None)
            .unwrap();

        let bundle = build_bundle(
            &proj,
            NewMemory {
                r#type: MemoryType::Note,
                body: "Semantic memory for fake provider test.",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        store.record_memory(&bundle).unwrap();

        // Embed with the fake provider so the vector index is populated.
        let cfg = EmbeddingsConfig {
            provider: "fake".into(),
            model: None,
            dimensions: Some(64),
        };
        let provider = build_provider(&cfg).unwrap();
        vestige_engine::embed::embed_all(
            &mut store,
            &proj,
            &*provider,
            &[vestige_core::RepresentationDepth::OneLiner],
            false,
        )
        .unwrap();

        let mut app = app_with(MemoriesTabState::default());
        app.memories.filter_text = "semantic".into();
        app.search_mode = SearchMode::Semantic;
        reload_list(&mut app, &store, &proj, Some(&*provider)).unwrap();
        // Fake provider always returns a cosine-similar result, so the list
        // must be non-empty.
        assert!(
            !app.memories.items.is_empty(),
            "semantic search with fake provider should find results"
        );
    }
}
