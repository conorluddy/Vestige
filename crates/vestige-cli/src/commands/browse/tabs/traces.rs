//! Traces tab — list pane + detail pane + replay diff.
//!
//! Read-only over `query_events` (traces are append-only audit).
//! `p` triggers a replay through `vestige_engine::replay_trace`; the
//! resulting `ReplayResult` is cached on the tab state and rendered in
//! place of the default detail until the user moves the cursor.
//!
//! M6 does not provide an embedding provider — semantic and hybrid
//! original modes will surface `provider_match=false` and a banner.
//! Lexical replays work without a provider.

use std::str::FromStr;

use anyhow::Result;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use vestige_core::{ProjectId, TraceId};
use vestige_engine::{list_traces, replay_trace, Caller, ListFilters, TraceCard, TraceDetail};
use vestige_store::Store;

use crate::commands::browse::app::App;

const LIST_LIMIT: u32 = 200;

// === PUBLIC API ===

pub fn reload_list(app: &mut App, store: &Store, project_id: &ProjectId) -> Result<()> {
    let filters = ListFilters {
        limit: LIST_LIMIT,
        ..ListFilters::default()
    };
    let state = &mut app.traces;
    state.load_error = None;
    match list_traces(store, project_id, &filters) {
        Ok(rows) => {
            state.items = rows;
            if state.selected >= state.items.len() {
                state.selected = state.items.len().saturating_sub(1);
            }
        }
        Err(e) => {
            state.items.clear();
            state.load_error = Some(format!("load failed: {e}"));
        }
    }
    refresh_detail(app, store, project_id)?;
    Ok(())
}

pub fn refresh_detail(app: &mut App, store: &Store, project_id: &ProjectId) -> Result<()> {
    let state = &mut app.traces;
    state.replay = None;
    let Some(id_str) = state.selected_id() else {
        state.detail = None;
        return Ok(());
    };
    let Ok(trace_id) = TraceId::from_str(id_str) else {
        state.detail = None;
        return Ok(());
    };
    match vestige_engine::get_trace(store, project_id, &trace_id) {
        Ok(detail) => state.detail = Some(detail),
        Err(e) => {
            state.detail = None;
            state.load_error = Some(format!("detail load failed: {e}"));
        }
    }
    Ok(())
}

/// Re-run the selected trace and stash the diff. M6 passes `provider=None`
/// — semantic/hybrid traces will surface `provider_match=false` and the
/// banner; lexical traces replay fine.
pub fn replay_selected(app: &mut App, store: &Store, project_id: &ProjectId) -> Result<()> {
    let Some(id_str) = app.traces.selected_id().map(|s| s.to_string()) else {
        return Ok(());
    };
    let Ok(trace_id) = TraceId::from_str(&id_str) else {
        return Ok(());
    };
    match replay_trace(store, None, project_id, &trace_id, Caller::Cli) {
        Ok(result) => {
            app.traces.replay = Some(result);
        }
        Err(e) => {
            app.traces.replay = None;
            app.traces.load_error = Some(format!("replay failed: {e}"));
        }
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(err) = &app.traces.load_error {
        let p = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    }
    if app.traces.items.is_empty() {
        draw_empty(frame, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    draw_list(frame, chunks[0], app);
    draw_detail(frame, chunks[1], app);
}

// === PRIVATE ===

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.traces.items.iter().map(row_for_trace).collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Traces ({})", app.traces.items.len()));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(app.traces.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn row_for_trace(card: &TraceCard) -> ListItem<'_> {
    let kind_style = match card.kind.as_str() {
        "search" => Style::default().fg(Color::Yellow),
        "expand" => Style::default().fg(Color::Blue),
        "context" => Style::default().fg(Color::Cyan),
        _ => Style::default(),
    };
    let caller_style = match card.caller.as_str() {
        "cli" => Style::default().fg(Color::Green),
        "mcp" => Style::default().fg(Color::Magenta),
        _ => Style::default(),
    };
    let query_preview = card
        .query
        .as_deref()
        .map(|q| preview(q, 50))
        .unwrap_or_else(|| "—".into());
    let mode = card.mode.as_deref().unwrap_or("-");
    let line = Line::from(vec![
        Span::styled(format!("{:<7}", card.kind), kind_style),
        Span::raw(" "),
        Span::styled(format!("{caller:<3}", caller = card.caller), caller_style),
        Span::raw(" "),
        Span::styled(format!("{mode:<7}"), Style::default().fg(Color::Gray)),
        Span::raw(" "),
        Span::styled(
            format!("{:>3} ", card.result_count),
            Style::default().fg(Color::Gray),
        ),
        Span::raw(query_preview),
    ]);
    ListItem::new(line)
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let title = if app.traces.replay.is_some() {
        "Detail · replay diff"
    } else {
        "Detail"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(replay) = &app.traces.replay {
        draw_replay(frame, inner, replay);
        return;
    }

    let Some(detail) = &app.traces.detail else {
        let p = Paragraph::new("(no selection)")
            .style(Style::default().add_modifier(Modifier::DIM))
            .alignment(Alignment::Center);
        frame.render_widget(p, inner);
        return;
    };

    let mut lines = trace_header(detail);

    if let Some(query) = &detail.query {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "query",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(query.clone()));
    }

    if let Some(params) = &detail.params {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "params",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(
            serde_json::to_string_pretty(params).unwrap_or_else(|_| params.to_string()),
        ));
    }

    if let Some(provider) = &detail.provider {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "provider",
            Style::default().fg(Color::Gray),
        )));
        let model = detail.provider_model.as_deref().unwrap_or("");
        lines.push(Line::from(format!("{provider} / {model}")));
    }

    if !detail.result_ids.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("results ({})", detail.result_count),
            Style::default().fg(Color::Gray),
        )));
        for (i, id) in detail.result_ids.iter().enumerate() {
            let score = detail.result_scores.get(i).copied().unwrap_or(0.0);
            lines.push(Line::from(format!("  {score:>7.3}  {id}")));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "p replay",
        Style::default().fg(Color::Gray),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn trace_header(detail: &TraceDetail) -> Vec<Line<'static>> {
    let mode = detail.mode_resolved.clone().unwrap_or_else(|| "-".into());
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            detail.trace_id.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(detail.kind.clone(), Style::default().fg(Color::Gray)),
        Span::raw("/"),
        Span::raw(mode),
        Span::raw("  "),
        Span::styled(detail.caller.clone(), Style::default().fg(Color::Magenta)),
        Span::raw("  "),
        Span::styled(
            format!("{} ms", detail.latency_ms),
            Style::default().fg(Color::Gray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        detail.created_at.clone(),
        Style::default().fg(Color::Gray),
    )));
    lines
}

fn draw_replay(frame: &mut Frame, area: Rect, replay: &vestige_engine::ReplayResult) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("replay of ", Style::default().fg(Color::Gray)),
        Span::styled(
            replay.trace_id.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        format!("new trace: {}", replay.replay_trace_id),
        Style::default().fg(Color::Gray),
    )));
    if !replay.provider_match {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "⚠  provider mismatch — replay re-ran with the current provider",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if replay.mode_fallback {
        lines.push(Line::from(Span::styled(
            "⚠  mode fallback — original mode unavailable, used lexical",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines.push(diff_header("added", replay.diff.added.len(), Color::Green));
    for id in &replay.diff.added {
        lines.push(Line::from(format!("  + {id}")));
    }
    lines.push(Line::from(""));
    lines.push(diff_header(
        "removed",
        replay.diff.removed.len(),
        Color::Red,
    ));
    for id in &replay.diff.removed {
        lines.push(Line::from(format!("  - {id}")));
    }
    lines.push(Line::from(""));
    lines.push(diff_header(
        "score changes",
        replay.diff.score_changes.len(),
        Color::Yellow,
    ));
    for change in &replay.diff.score_changes {
        lines.push(Line::from(format!(
            "  Δ {:+.3}  {}",
            change.delta, change.id
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("corpus size: {}", replay.corpus_size),
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc / move — back to detail",
        Style::default().fg(Color::Gray),
    )));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn diff_header(label: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("{label} ({count})"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn draw_empty(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "No traces yet.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Traces are written every time you run search, expand, or context."),
        Line::from(""),
        Line::from(Span::styled(
            "Try a search to populate this tab:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  vestige search \"…\"",
            Style::default().fg(Color::Cyan),
        )),
    ];
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
    use crate::commands::browse::app::{Counts, Tab, TracesTabState};
    use ratatui::{backend::TestBackend, Terminal};

    fn card(kind: &str, caller: &str, query: &str, results: u32) -> TraceCard {
        TraceCard {
            trace_id: vestige_core::TraceId::new().to_string(),
            kind: kind.into(),
            mode: Some("hybrid".into()),
            query: Some(query.into()),
            result_count: results,
            latency_ms: 12,
            caller: caller.into(),
            created_at: "2026-05-08T10:00:00Z".into(),
        }
    }

    fn app_with(state: TracesTabState) -> App {
        let mut a = App::new(Tab::Traces, Counts::default(), "p".into());
        a.traces = state;
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
    fn empty_state_explains_how_to_populate() {
        let app = app_with(TracesTabState::default());
        let out = render(&app);
        assert!(out.contains("No traces yet"));
        assert!(out.contains("vestige search"));
    }

    #[test]
    fn populated_list_shows_kind_caller_query() {
        let s = TracesTabState {
            items: vec![
                card("search", "cli", "ratatui browser", 7),
                card("expand", "mcp", "(no query)", 0),
            ],
            ..Default::default()
        };
        let app = app_with(s);
        let out = render(&app);
        assert!(out.contains("Traces (2)"));
        assert!(out.contains("search"));
        assert!(out.contains("expand"));
        assert!(out.contains("ratatui browser"));
        assert!(out.contains("cli"));
        assert!(out.contains("mcp"));
    }

    #[test]
    fn detail_renders_query_provider_results_and_replay_hint() {
        let s = TracesTabState {
            items: vec![card("search", "cli", "test", 2)],
            detail: Some(TraceDetail {
                trace_id: "trace_01HX0000000000000000000ABC".into(),
                kind: "search".into(),
                mode_requested: Some("hybrid".into()),
                mode_resolved: Some("hybrid".into()),
                query: Some("ratatui".into()),
                params: None,
                caller: "cli".into(),
                provider: Some("fake".into()),
                provider_model: Some("test-model".into()),
                result_ids: vec!["mem_01HX0000000000000000000ABC".into()],
                result_scores: vec![0.42],
                result_count: 1,
                latency_ms: 5,
                created_at: "2026-05-08T10:00:00Z".into(),
            }),
            ..Default::default()
        };
        let app = app_with(s);
        let out = render(&app);
        assert!(out.contains("Detail"));
        assert!(out.contains("trace_01HX"));
        assert!(out.contains("query"));
        assert!(out.contains("ratatui"));
        assert!(out.contains("provider"));
        assert!(out.contains("fake / test-model"));
        assert!(out.contains("results (1)"));
        assert!(out.contains("0.420"));
        assert!(out.contains("p replay"));
    }

    #[test]
    fn end_to_end_replay_against_real_store() {
        use tempfile::TempDir;
        use vestige_core::{build_bundle, NewMemory};

        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
        let proj = vestige_core::ProjectId::from_slug("traces-tab-test");
        store
            .ensure_project(&proj, "Traces Tab", Some("/tmp/test"), None)
            .unwrap();

        // Record a memory so the search has results.
        let bundle = build_bundle(
            &proj,
            NewMemory {
                r#type: vestige_core::MemoryType::Note,
                body: "Hybrid recall is the goal.",
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        store.record_memory(&bundle).unwrap();

        // Run a search via the engine so a trace lands.
        let cfg = vestige_engine::TracesConfig::default();
        let _ = vestige_engine::search::search_lexical(
            &store,
            &proj,
            "hybrid",
            None,
            10,
            vestige_engine::Caller::Cli,
            &cfg,
        )
        .unwrap();

        let mut app = app_with(TracesTabState::default());
        reload_list(&mut app, &store, &proj).unwrap();
        assert!(!app.traces.items.is_empty(), "expected one trace");

        replay_selected(&mut app, &store, &proj).unwrap();
        let replay = app.traces.replay.as_ref().expect("replay populated");
        assert_eq!(
            replay.original.result_ids.len(),
            replay.current.result_ids.len()
        );
        // Lexical replay → provider_match should be true (no provider needed).
        assert!(replay.provider_match);
    }

    #[test]
    fn replay_diff_renders_added_removed_score_changes() {
        let s = TracesTabState {
            items: vec![card("search", "cli", "test", 2)],
            detail: None,
            replay: Some(vestige_engine::ReplayResult {
                trace_id: "trace_01HX0000000000000000000ABC".into(),
                original: vestige_engine::ReplayResultSet {
                    result_ids: vec!["mem_a".into(), "mem_b".into()],
                    scores: vec![0.5, 0.3],
                },
                current: vestige_engine::ReplayResultSet {
                    result_ids: vec!["mem_a".into(), "mem_c".into()],
                    scores: vec![0.7, 0.2],
                },
                diff: vestige_engine::ReplayDiff {
                    added: vec!["mem_c".into()],
                    removed: vec!["mem_b".into()],
                    score_changes: vec![vestige_engine::ScoreChange {
                        id: "mem_a".into(),
                        delta: 0.2,
                    }],
                },
                provider_match: false,
                mode_fallback: false,
                replay_trace_id: "trace_01HX0000000000000000000NEW".into(),
                corpus_size: 42,
            }),
            ..Default::default()
        };
        let app = app_with(s);
        let out = render(&app);
        assert!(out.contains("replay diff"));
        assert!(out.contains("provider mismatch"));
        assert!(out.contains("added (1)"));
        assert!(out.contains("+ mem_c"));
        assert!(out.contains("removed (1)"));
        assert!(out.contains("- mem_b"));
        assert!(out.contains("score changes (1)"));
        assert!(out.contains("+0.200"));
        assert!(out.contains("corpus size: 42"));
    }
}
