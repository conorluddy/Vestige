//! Draw the browser frame.
//!
//! Layout (top → bottom):
//! 1. Tab bar (1 row): three tab labels with counts in brackets.
//! 2. Body (fills): centred placeholder for the active tab.
//! 3. Status line (1 row): project name on the left, key hint on the right.
//!
//! `NO_COLOR` is honoured: when the env var is set, styles fall back to
//! reverse-video for the active tab and no foreground colour anywhere.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use super::app::{App, CommandPalette, Modal, Tab};

// === PUBLIC API ===

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // body
            Constraint::Length(1), // status
        ])
        .split(area);

    draw_tab_bar(frame, chunks[0], app);
    draw_body(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);

    if app.help_open {
        draw_help(frame, area);
    }
    if let Some(modal) = &app.modal {
        draw_modal(frame, area, modal);
    }
}

// === PRIVATE ===

fn draw_tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let mut spans: Vec<Span> = Vec::new();
    for (i, tab) in [Tab::Memories, Tab::Candidates, Tab::Traces]
        .iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let count = match tab {
            Tab::Memories => app.counts.memories_active,
            Tab::Candidates => app.counts.candidates_pending,
            Tab::Traces => app.counts.traces,
        };
        let label = format!("[{}({})]", tab.label(), count);
        let style = if *tab == app.tab {
            active_tab_style(no_color)
        } else {
            Style::default()
        };
        spans.push(Span::styled(label, style));
    }
    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &App) {
    match app.tab {
        Tab::Memories => super::tabs::memories::draw(frame, area, app),
        Tab::Candidates => super::tabs::candidates::draw(frame, area, app),
        Tab::Traces => super::tabs::traces::draw(frame, area, app),
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    // When the palette is open, the status line is taken over by the prompt.
    if let Some(palette) = &app.palette {
        draw_palette(frame, area, palette);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(40)])
        .split(area);

    // A flash overrides the project label until the user issues another input
    // (the dispatcher clears it on the next non-trivial action).
    let left_text = if let Some(flash) = &app.status_flash {
        flash.text.clone()
    } else {
        // Always show live counts so the user knows the size of each tab
        // without flipping through them.
        format!(
            "Vestige · {} · [Mem {} · Cand {} · Trc {}]",
            app.project_name,
            app.memories.items.len(),
            app.candidates.items.len(),
            app.traces.items.len(),
        )
    };
    let left_style = match &app.status_flash {
        Some(f) if f.is_error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Some(_) => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        None => Style::default(),
    };
    let left = Paragraph::new(Span::styled(left_text, left_style)).alignment(Alignment::Left);
    let right = Paragraph::new("Tab switch · ? help · q quit").alignment(Alignment::Right);
    frame.render_widget(left, chunks[0]);
    frame.render_widget(right, chunks[1]);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centred_rect(70, 90, area);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from("Vestige Browser — keymap"),
        Line::from(""),
        Line::from("  Tab / Shift-Tab   cycle tabs"),
        Line::from("  j / k or ↓ / ↑    move selection"),
        Line::from("  g / G             first / last"),
        Line::from("  Ctrl-d / Ctrl-u   half-page down / up"),
        Line::from("  /                 focus filter"),
        Line::from("  :                 command palette (:help inside it lists commands)"),
        Line::from("  w                 why — provenance walk"),
        Line::from("  s                 sources — typed receipts"),
        Line::from("  t                 traces-of — which queries returned this"),
        Line::from("  f                 forget memory (soft-delete, with confirm)"),
        Line::from("  r                 restore soft-deleted memory (with confirm)"),
        Line::from("  a                 approve candidate (with confirm)"),
        Line::from("  R                 reject candidate (with reason prompt)"),
        Line::from("  p                 replay trace — diff against current store"),
        Line::from("  Esc               close overlay / clear filter / back"),
        Line::from("  ?                 toggle this help"),
        Line::from("  q / Ctrl-c        quit"),
    ];
    let block = Block::default().borders(Borders::ALL).title("Help");
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal) {
    let popup = centred_rect(60, 50, area);
    frame.render_widget(Clear, popup);
    let title = if modal.is_prompt() {
        format!("Prompt: {} reason", modal.verb().to_lowercase())
    } else {
        format!("Confirm {}", modal.verb().to_lowercase())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = match modal {
        Modal::PromptRejectReason { id, buffer } => vec![
            Line::from(""),
            Line::from(Span::styled(
                "Reject candidate",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(id.as_str().to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Reason (empty = unspecified):",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                format!("> {buffer}_"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Enter - submit     Esc - cancel",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "presets: duplicate | wrong | not_durable | too_noisy | stale",
                Style::default().fg(Color::Gray),
            )),
        ],
        _ => {
            let subject = modal.subject_id();
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("{} {}", modal.verb(), subject_label(modal)),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(subject),
                Line::from(""),
                Line::from(Span::styled(
                    "y - yes     n / Enter / Esc - no",
                    Style::default().fg(Color::Gray),
                )),
            ]
        }
    };
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn subject_label(modal: &Modal) -> &'static str {
    match modal {
        Modal::ConfirmForget(_) | Modal::ConfirmRestore(_) => "memory",
        Modal::ConfirmApprove(_) | Modal::PromptRejectReason { .. } => "candidate",
    }
}

fn draw_palette(frame: &mut Frame, area: Rect, palette: &CommandPalette) {
    let text = match &palette.error {
        Some(err) => format!(":{}    [{err}]", palette.buffer),
        None => format!(":{}_", palette.buffer),
    };
    let style = if palette.error.is_some() {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    };
    let paragraph = Paragraph::new(Span::styled(text, style)).alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

fn active_tab_style(no_color: bool) -> Style {
    if no_color {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    }
}

fn centred_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let popup_w = area.width * pct_x / 100;
    let popup_h = area.height * pct_y / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(popup_w)) / 2,
        y: area.y + (area.height.saturating_sub(popup_h)) / 2,
        width: popup_w,
        height: popup_h,
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::browse::app::{Counts, Tab};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn draws_tab_bar_with_counts() {
        let counts = Counts {
            memories_active: 47,
            candidates_pending: 3,
            traces: 184,
        };
        let app = App::new(Tab::Memories, counts, "proj_test".into());
        let out = render(&app);
        assert!(out.contains("[Memories(47)]"), "got: {out}");
        assert!(out.contains("[Candidates(3)]"));
        assert!(out.contains("[Traces(184)]"));
    }

    #[test]
    fn memories_tab_renders_empty_state_when_no_items() {
        // The Memories tab now owns its body. Without items it draws the
        // rich empty-state copy from `tabs::memories`. Tab bar still shows
        // the startup count from `Counts`.
        let counts = Counts {
            memories_active: 0,
            candidates_pending: 3,
            traces: 184,
        };
        let app = App::new(Tab::Memories, counts, "p".into());
        let out = render(&app);
        assert!(out.contains("No memories yet"), "got: {out}");
    }

    #[test]
    fn each_tab_renders_its_real_empty_state() {
        let counts = Counts::default();
        let mut app = App::new(Tab::Memories, counts, "p".into());
        assert!(render(&app).contains("No memories yet"));
        app.tab = Tab::Candidates;
        assert!(render(&app).contains("Inbox empty"));
        app.tab = Tab::Traces;
        assert!(render(&app).contains("No traces yet"));
    }

    #[test]
    fn status_line_shows_project_and_keys() {
        let app = App::new(Tab::Memories, Counts::default(), "proj_demo".into());
        let out = render(&app);
        assert!(out.contains("Vestige · proj_demo"));
        assert!(out.contains("Tab switch · ? help · q quit"));
    }

    #[test]
    fn help_overlay_renders_when_open() {
        let mut app = App::new(Tab::Memories, Counts::default(), "p".into());
        app.help_open = true;
        let out = render(&app);
        assert!(out.contains("Help"), "no Help title: {out}");
        assert!(out.contains("cycle tabs"), "got: {out}");
        assert!(out.contains("move selection"));
        assert!(out.contains("why"));
        assert!(out.contains("sources"));
        assert!(out.contains("traces-of"));
        assert!(out.contains("quit"));
    }

    #[test]
    fn active_tab_styled_differently_from_inactive() {
        let counts = Counts {
            memories_active: 1,
            candidates_pending: 1,
            traces: 1,
        };
        let app = App::new(Tab::Memories, counts, "p".into());
        let backend = TestBackend::new(120, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();

        // Find the M of "[Memories(1)]" — that cell must be styled differently
        // from the M of "[Candidates(1)]" (which contains no M but is unstyled).
        // We compare the bold modifier on the first character of each label.
        let mem_cell = find_cell(buffer, "M").expect("Memories label present");
        let cand_cell = find_cell(buffer, "C").expect("Candidates label present");
        assert_ne!(
            mem_cell.style(),
            cand_cell.style(),
            "active tab should differ in style from inactive"
        );
    }

    #[test]
    fn confirm_modal_renders_with_id_and_keys() {
        let mut app = App::new(Tab::Memories, Counts::default(), "p".into());
        let mem = vestige_core::MemoryId::new();
        let mem_str = mem.as_str().to_string();
        app.modal = Some(Modal::ConfirmForget(mem));
        let out = render(&app);
        assert!(out.contains("Confirm forget"), "title; got: {out}");
        assert!(out.contains("Forget memory"), "verb; got: {out}");
        assert!(
            out.contains(&mem_str[..16]),
            "id prefix should appear; got: {out}"
        );
        assert!(out.contains("y"), "y key prompt; got: {out}");
        assert!(out.contains("yes"));
        assert!(out.contains("Enter"));
        assert!(out.contains("Esc"));
    }

    #[test]
    fn restore_confirm_title_uses_correct_verb() {
        let mut app = App::new(Tab::Memories, Counts::default(), "p".into());
        app.modal = Some(Modal::ConfirmRestore(vestige_core::MemoryId::new()));
        let out = render(&app);
        assert!(out.contains("Confirm restore"), "got: {out}");
        assert!(out.contains("Restore memory"));
    }

    #[test]
    fn palette_renders_in_status_line() {
        let mut app = App::new(Tab::Memories, Counts::default(), "p".into());
        app.palette = Some(super::super::app::CommandPalette {
            buffer: "goto mem_01HX".into(),
            error: None,
        });
        let out = render(&app);
        assert!(out.contains(":goto mem_01HX"), "got: {out}");
    }

    #[test]
    fn palette_error_renders_in_red() {
        let mut app = App::new(Tab::Memories, Counts::default(), "p".into());
        app.palette = Some(super::super::app::CommandPalette {
            buffer: "kind bogus".into(),
            error: Some("unknown memory type: bogus".into()),
        });
        let out = render(&app);
        assert!(out.contains("unknown memory type"), "got: {out}");
    }

    #[test]
    fn status_line_shows_live_counts() {
        let counts = Counts {
            memories_active: 47,
            candidates_pending: 3,
            traces: 184,
        };
        let mut app = App::new(Tab::Memories, counts, "proj_demo".into());
        // Populate item lengths to drive the live count
        for _ in 0..5 {
            app.memories
                .items
                .push(vestige_core::MemoryCard {
                    id: vestige_core::MemoryId::new(),
                    r#type: vestige_core::MemoryType::Note,
                    status: vestige_core::MemoryStatus::Active,
                    title: "x".into(),
                    one_liner: "y".into(),
                    importance: 0.5,
                    created_at: time::OffsetDateTime::now_utc(),
                    updated_at: time::OffsetDateTime::now_utc(),
                    available_depths: vec![],
                });
        }
        let out = render(&app);
        assert!(out.contains("Mem 5"), "live mem count; got: {out}");
        assert!(out.contains("Cand 0"));
        assert!(out.contains("Trc 0"));
    }

    #[test]
    fn status_flash_overrides_project_label() {
        let mut app = App::new(Tab::Memories, Counts::default(), "proj_demo".into());
        app.status_flash = Some(super::super::app::StatusFlash {
            text: "Forgot mem_01HX0000000000000000000ABC".into(),
            is_error: false,
        });
        let out = render(&app);
        assert!(out.contains("Forgot mem_01HX"), "got: {out}");
        assert!(
            !out.contains("Vestige · proj_demo"),
            "flash hides project label"
        );
    }

    #[test]
    fn no_color_uses_reverse_instead_of_fg_bg() {
        // Set NO_COLOR for the duration of the render. SAFETY: tests in this
        // module are not run concurrently with anything that reads NO_COLOR.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let counts = Counts {
            memories_active: 1,
            ..Default::default()
        };
        let app = App::new(Tab::Memories, counts, "p".into());
        let backend = TestBackend::new(120, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mem_cell = find_cell(buffer, "M").expect("Memories label present");
        let style = mem_cell.style();
        unsafe { std::env::remove_var("NO_COLOR") };
        assert!(
            style.add_modifier.contains(Modifier::REVERSED),
            "NO_COLOR active tab should use REVERSED modifier; got {:?}",
            style
        );
        let chromatic = |c: Option<Color>| matches!(c, Some(c) if c != Color::Reset);
        assert!(
            !chromatic(style.fg),
            "NO_COLOR should not set a chromatic fg; got {:?}",
            style.fg
        );
        assert!(
            !chromatic(style.bg),
            "NO_COLOR should not set a chromatic bg; got {:?}",
            style.bg
        );
    }

    fn find_cell<'a>(
        buffer: &'a ratatui::buffer::Buffer,
        needle: &str,
    ) -> Option<&'a ratatui::buffer::Cell> {
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol() == needle {
                    return Some(cell);
                }
            }
        }
        None
    }
}
