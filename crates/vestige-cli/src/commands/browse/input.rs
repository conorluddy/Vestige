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
    map_key(key, app)
}

fn is_press(key: &KeyEvent) -> bool {
    use ratatui::crossterm::event::KeyEventKind;
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn map_key(key: &KeyEvent, app: &App) -> Action {
    // Modal swallows input until resolved. Prompt modals accept text;
    // confirm modals only accept y/n.
    if let Some(modal) = &app.modal {
        if modal.is_prompt() {
            return match key.code {
                KeyCode::Esc => Action::CloseOverlay,
                KeyCode::Enter => Action::PromptSubmit,
                KeyCode::Backspace => Action::PromptBackspace,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
                KeyCode::Char(c) => Action::PromptChar(c),
                _ => Action::None,
            };
        }
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::ConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Enter => Action::ConfirmNo,
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            _ => Action::None,
        };
    }
    if app.help_open {
        return match key.code {
            KeyCode::Esc => Action::CloseOverlay,
            KeyCode::Char('?') => Action::ToggleHelp,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            _ => Action::None,
        };
    }
    if app.tab == super::app::Tab::Memories && app.memories.filter_focused {
        return map_filter_key(key);
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Tab => Action::NextTab,
        KeyCode::BackTab => Action::PrevTab,
        // Memories-tab navigation. Surfaced unconditionally for M2; the
        // dispatcher in `mod.rs` only acts on these when the active tab is
        // Memories. M5/M6 will route by `app.tab`.
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        KeyCode::Char('g') => Action::MoveTop,
        KeyCode::Char('G') => Action::MoveBottom,
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::HalfPageDown,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::HalfPageUp,
        KeyCode::Char('/') => Action::OpenFilter,
        // Provenance sub-views — only meaningful on the Memories tab. The
        // dispatcher in `mod.rs` no-ops these on other tabs.
        KeyCode::Char('w') => Action::ShowWhy,
        KeyCode::Char('s') => Action::ShowSources,
        KeyCode::Char('t') => Action::ShowTracesOf,
        // Mutations — dispatcher checks the active tab + selected status:
        //   memories: f → forget (active), r → restore (deleted)
        //   candidates: a → approve, R (shift+r) → reject prompt
        KeyCode::Char('f') => Action::RequestForget,
        KeyCode::Char('r') => {
            if app.tab == super::app::Tab::Candidates && key.modifiers.contains(KeyModifiers::SHIFT)
            {
                Action::RequestReject
            } else {
                Action::RequestRestore
            }
        }
        KeyCode::Char('R') => Action::RequestReject,
        KeyCode::Char('a') => Action::RequestApprove,
        KeyCode::Esc => Action::CloseOverlay,
        _ => Action::None,
    }
}

fn map_filter_key(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => Action::CloseOverlay,
        KeyCode::Backspace => Action::FilterBackspace,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char(c) => Action::FilterChar(c),
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

    #[test]
    fn j_and_down_move_down() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('j'), KeyModifiers::NONE), &a),
            Action::MoveDown
        );
        assert_eq!(
            map_event(&press(KeyCode::Down, KeyModifiers::NONE), &a),
            Action::MoveDown
        );
    }

    #[test]
    fn k_and_up_move_up() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('k'), KeyModifiers::NONE), &a),
            Action::MoveUp
        );
        assert_eq!(
            map_event(&press(KeyCode::Up, KeyModifiers::NONE), &a),
            Action::MoveUp
        );
    }

    #[test]
    fn g_and_shift_g_jump_top_and_bottom() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('g'), KeyModifiers::NONE), &a),
            Action::MoveTop
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('G'), KeyModifiers::SHIFT), &a),
            Action::MoveBottom
        );
    }

    #[test]
    fn ctrl_d_and_ctrl_u_half_page() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('d'), KeyModifiers::CONTROL), &a),
            Action::HalfPageDown
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('u'), KeyModifiers::CONTROL), &a),
            Action::HalfPageUp
        );
    }

    #[test]
    fn slash_opens_filter() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('/'), KeyModifiers::NONE), &a),
            Action::OpenFilter
        );
    }

    #[test]
    fn a_and_shift_r_request_candidate_mutations() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('a'), KeyModifiers::NONE), &a),
            Action::RequestApprove
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('R'), KeyModifiers::SHIFT), &a),
            Action::RequestReject
        );
    }

    #[test]
    fn prompt_modal_text_input_dispatch() {
        let mut a = app(false);
        a.modal = Some(super::super::app::Modal::PromptRejectReason {
            id: vestige_core::CandidateId::generate(),
            buffer: String::new(),
        });
        assert_eq!(
            map_event(&press(KeyCode::Char('d'), KeyModifiers::NONE), &a),
            Action::PromptChar('d')
        );
        assert_eq!(
            map_event(&press(KeyCode::Backspace, KeyModifiers::NONE), &a),
            Action::PromptBackspace
        );
        assert_eq!(
            map_event(&press(KeyCode::Enter, KeyModifiers::NONE), &a),
            Action::PromptSubmit
        );
        assert_eq!(
            map_event(&press(KeyCode::Esc, KeyModifiers::NONE), &a),
            Action::CloseOverlay
        );
        // y/n are NOT confirm keys in a prompt — they're characters.
        assert_eq!(
            map_event(&press(KeyCode::Char('y'), KeyModifiers::NONE), &a),
            Action::PromptChar('y')
        );
    }

    #[test]
    fn f_and_r_request_mutations() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('f'), KeyModifiers::NONE), &a),
            Action::RequestForget
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('r'), KeyModifiers::NONE), &a),
            Action::RequestRestore
        );
    }

    #[test]
    fn modal_swallows_input_until_resolved() {
        let mut a = app(false);
        a.modal = Some(super::super::app::Modal::ConfirmForget(
            vestige_core::MemoryId::new(),
        ));
        assert_eq!(
            map_event(&press(KeyCode::Char('y'), KeyModifiers::NONE), &a),
            Action::ConfirmYes
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('n'), KeyModifiers::NONE), &a),
            Action::ConfirmNo
        );
        assert_eq!(
            map_event(&press(KeyCode::Enter, KeyModifiers::NONE), &a),
            Action::ConfirmNo,
            "Enter should be safe — defaults to No"
        );
        assert_eq!(
            map_event(&press(KeyCode::Esc, KeyModifiers::NONE), &a),
            Action::CloseOverlay
        );
        // Tab is otherwise NextTab; the modal must swallow it.
        assert_eq!(
            map_event(&press(KeyCode::Tab, KeyModifiers::NONE), &a),
            Action::None
        );
    }

    #[test]
    fn w_s_t_map_to_provenance_subviews() {
        let a = app(false);
        assert_eq!(
            map_event(&press(KeyCode::Char('w'), KeyModifiers::NONE), &a),
            Action::ShowWhy
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('s'), KeyModifiers::NONE), &a),
            Action::ShowSources
        );
        assert_eq!(
            map_event(&press(KeyCode::Char('t'), KeyModifiers::NONE), &a),
            Action::ShowTracesOf
        );
    }

    #[test]
    fn filter_focused_chars_become_filter_input() {
        let mut a = app(false);
        a.memories.filter_focused = true;
        assert_eq!(
            map_event(&press(KeyCode::Char('a'), KeyModifiers::NONE), &a),
            Action::FilterChar('a')
        );
        // j/k must be treated as text when filtering, not as navigation
        assert_eq!(
            map_event(&press(KeyCode::Char('j'), KeyModifiers::NONE), &a),
            Action::FilterChar('j')
        );
        assert_eq!(
            map_event(&press(KeyCode::Backspace, KeyModifiers::NONE), &a),
            Action::FilterBackspace
        );
        assert_eq!(
            map_event(&press(KeyCode::Esc, KeyModifiers::NONE), &a),
            Action::CloseOverlay
        );
    }
}
