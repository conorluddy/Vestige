//! Map a `crossterm::event::Event` to an [`Action`].
//!
//! Pure function — no I/O. Held separate from `app.rs` so input handling can be
//! tested without constructing the full `App`. The event loop in `mod.rs`
//! reads events via `crossterm::event::read()` then calls [`map_event`].

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::app::{Action, App};

/// Map a single crossterm event to an [`Action`] in the context of the current
/// `App` state. `help_open` is the only state that gates input — when the help
/// overlay is up, `Esc` and `?` close it; everything else is a no-op.
pub fn map_event(event: &Event, app: &App) -> Action {
    let Event::Key(key) = event else {
        return Action::None;
    };
    // crossterm 0.29 fires KeyEventKind::Release on some terminals — only
    // act on Press to avoid double-fires. Older terminals always send Press.
    if !is_press(key) {
        return Action::None;
    }
    map_key(key, app.help_open)
}

fn is_press(key: &KeyEvent) -> bool {
    use ratatui::crossterm::event::KeyEventKind;
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn map_key(key: &KeyEvent, help_open: bool) -> Action {
    if help_open {
        return match key.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Char('?') => Action::ToggleHelp,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            _ => Action::None,
        };
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Tab => Action::NextTab,
        KeyCode::BackTab => Action::PrevTab,
        _ => Action::None,
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::browse::app::{Counts, Tab};
    use ratatui::crossterm::event::KeyEventKind;

    fn press(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        })
    }

    fn app(help_open: bool) -> App {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.help_open = help_open;
        a
    }

    #[test]
    fn tab_advances_tab() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Tab, KeyModifiers::NONE), &a),
            Action::NextTab
        );
    }

    #[test]
    fn back_tab_reverses() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::BackTab, KeyModifiers::NONE), &a),
            Action::PrevTab
        );
    }

    #[test]
    fn q_quits() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('q'), KeyModifiers::NONE), &a),
            Action::Quit
        );
    }

    #[test]
    fn ctrl_c_quits() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('c'), KeyModifiers::CONTROL), &a),
            Action::Quit
        );
    }

    #[test]
    fn qmark_toggles_help() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('?'), KeyModifiers::NONE), &a),
            Action::ToggleHelp
        );
    }

    #[test]
    fn esc_closes_help_when_open() {
        let a = app(true);
        assert_eq!(
            map_event(&press(KeyCode::Esc, KeyModifiers::NONE), &a),
            Action::CloseOverlay
        );
    }

    #[test]
    fn tab_is_noop_when_help_open() {
        let a = app(true);
        assert_eq!(
            map_event(&press(KeyCode::Tab, KeyModifiers::NONE), &a),
            Action::None
        );
    }

    #[test]
    fn release_events_ignored() {
        let a = app(false);
        let release = Event::Key(KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        });
        assert_eq!(map_event(&release, &a), Action::None);
    }

    #[test]
    fn resize_event_is_noop() {
        let a = app(false);
        assert_eq!(map_event(&Event::Resize(80, 24), &a), Action::None);
    }
}
