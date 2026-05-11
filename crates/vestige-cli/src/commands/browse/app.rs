//! Pure application state for `vestige browse`.
//!
//! No I/O lives here — only the data the UI reads and the cheap mutators that
//! drive it. The event loop calls `App::handle(action)` after `event` maps a
//! `crossterm::event::Event` to an [`Action`].

// === TYPES ===

/// Top-level tabs in the browser. Order is the cycling order for `Tab` /
/// `Shift-Tab`. Matches PRD §6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Memories,
    Candidates,
    Traces,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Memories => "Memories",
            Tab::Candidates => "Candidates",
            Tab::Traces => "Traces",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::Memories => Tab::Candidates,
            Tab::Candidates => Tab::Traces,
            Tab::Traces => Tab::Memories,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Memories => Tab::Traces,
            Tab::Candidates => Tab::Memories,
            Tab::Traces => Tab::Candidates,
        }
    }
}

/// One snapshot of the per-tab counts read at startup.
///
/// M1 reads these once when the browser opens. Later milestones move to
/// read-on-tab-switch when each tab gets a real list view.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub memories_active: i64,
    pub candidates_pending: i64,
    pub traces: i64,
}

/// Action produced by the event mapper. Consumed by [`App::handle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    NextTab,
    PrevTab,
    ToggleHelp,
    CloseOverlay,
    None,
}

/// The browser's full mutable state.
pub struct App {
    pub project_name: String,
    pub tab: Tab,
    pub counts: Counts,
    pub help_open: bool,
    pub should_quit: bool,
}

// === PUBLIC API ===

impl App {
    pub fn new(initial_tab: Tab, counts: Counts, project_name: String) -> Self {
        Self {
            project_name,
            tab: initial_tab,
            counts,
            help_open: false,
            should_quit: false,
        }
    }

    pub fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::NextTab => self.tab = self.tab.next(),
            Action::PrevTab => self.tab = self.tab.prev(),
            Action::ToggleHelp => self.help_open = !self.help_open,
            Action::CloseOverlay => self.help_open = false,
            Action::None => {}
        }
    }

    /// Count shown in the active tab's body placeholder.
    pub fn active_tab_count(&self) -> i64 {
        match self.tab {
            Tab::Memories => self.counts.memories_active,
            Tab::Candidates => self.counts.candidates_pending,
            Tab::Traces => self.counts.traces,
        }
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(Tab::Memories, Counts::default(), "test".into())
    }

    #[test]
    fn next_tab_cycles_memories_candidates_traces_memories() {
        let mut a = app();
        assert_eq!(a.tab, Tab::Memories);
        a.handle(Action::NextTab);
        assert_eq!(a.tab, Tab::Candidates);
        a.handle(Action::NextTab);
        assert_eq!(a.tab, Tab::Traces);
        a.handle(Action::NextTab);
        assert_eq!(a.tab, Tab::Memories);
    }

    #[test]
    fn prev_tab_wraps_from_memories_to_traces() {
        let mut a = app();
        a.handle(Action::PrevTab);
        assert_eq!(a.tab, Tab::Traces);
    }

    #[test]
    fn quit_sets_should_quit() {
        let mut a = app();
        assert!(!a.should_quit);
        a.handle(Action::Quit);
        assert!(a.should_quit);
    }

    #[test]
    fn toggle_help_flips_and_close_overlay_clears() {
        let mut a = app();
        a.handle(Action::ToggleHelp);
        assert!(a.help_open);
        a.handle(Action::CloseOverlay);
        assert!(!a.help_open);
    }

    #[test]
    fn active_tab_count_follows_selection() {
        let counts = Counts {
            memories_active: 7,
            candidates_pending: 3,
            traces: 99,
        };
        let mut a = App::new(Tab::Memories, counts, "p".into());
        assert_eq!(a.active_tab_count(), 7);
        a.handle(Action::NextTab);
        assert_eq!(a.active_tab_count(), 3);
        a.handle(Action::NextTab);
        assert_eq!(a.active_tab_count(), 99);
    }
}
