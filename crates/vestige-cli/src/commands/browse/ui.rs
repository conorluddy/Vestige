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

use super::app::{App, Tab};

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
        Tab::Candidates => draw_placeholder(frame, area, "Candidates", "M5"),
        Tab::Traces => draw_placeholder(frame, area, "Traces", "M6"),
    }
}

fn draw_placeholder(frame: &mut Frame, area: Rect, label: &str, when: &str) {
    let lines = vec![
        Line::from(Span::styled(
            format!("{label} tab"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Lands in {when}.")),
        Line::from(""),
        Line::from(Span::styled(
            "CLI alternatives for now:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            placeholder_cli_hint(label),
            Style::default().fg(Color::Cyan),
        )),
    ];
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    let centred = centre_vertically(area, 6);
    frame.render_widget(paragraph, centred);
}

fn placeholder_cli_hint(label: &str) -> &'static str {
    match label {
        "Candidates" => "vestige inbox · vestige approve · vestige reject",
        "Traces" => "vestige trace · vestige trace <id> · vestige trace replay <id>",
        _ => "",
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(40)])
        .split(area);

    let left = Paragraph::new(format!("Vestige · {}", app.project_name)).alignment(Alignment::Left);
    let right = Paragraph::new("Tab switch · ? help · q quit").alignment(Alignment::Right);
    frame.render_widget(left, chunks[0]);
    frame.render_widget(right, chunks[1]);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centred_rect(60, 60, area);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from("Vestige Browser — keymap"),
        Line::from(""),
        Line::from("  Tab / Shift-Tab   cycle tabs"),
        Line::from("  j / k or ↓ / ↑    move selection"),
        Line::from("  g / G             first / last"),
        Line::from("  Ctrl-d / Ctrl-u   half-page down / up"),
        Line::from("  /                 focus filter"),
        Line::from("  w                 why — provenance walk"),
        Line::from("  s                 sources — typed receipts"),
        Line::from("  t                 traces-of — which queries returned this"),
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

fn centre_vertically(area: Rect, height: u16) -> Rect {
    let top = area.height.saturating_sub(height) / 2;
    Rect {
        x: area.x,
        y: area.y + top,
        width: area.width,
        height: height.min(area.height),
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
    fn candidates_and_traces_placeholders_point_at_cli() {
        let counts = Counts {
            memories_active: 0,
            candidates_pending: 3,
            traces: 184,
        };
        let mut app = App::new(Tab::Candidates, counts, "p".into());
        let out = render(&app);
        assert!(out.contains("Candidates tab"));
        assert!(out.contains("Lands in M5"));
        assert!(out.contains("vestige inbox"));

        app.tab = Tab::Traces;
        let out = render(&app);
        assert!(out.contains("Traces tab"));
        assert!(out.contains("Lands in M6"));
        assert!(out.contains("vestige trace"));
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
