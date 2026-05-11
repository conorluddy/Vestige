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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    NextTab,
    PrevTab,
    ToggleHelp,
    CloseOverlay,
    // Memories tab list navigation. The event loop interprets these against
    // `App::memories` and re-fetches detail after the cursor moves.
    MoveDown,
    MoveUp,
    MoveTop,
    MoveBottom,
    HalfPageDown,
    HalfPageUp,
    // Filter input. `OpenFilter` focuses the prompt; `FilterChar`/`FilterBackspace`
    // edit it; `Esc` closes (handled via `CloseOverlay`).
    OpenFilter,
    FilterChar(char),
    FilterBackspace,
    None,
}

/// Per-tab state for the Memories tab. Owned by [`App`].
///
/// Holds the currently loaded list (`items`), the cursor (`selected`), the
/// scroll window into `items`, the optional filter, and a cached detail for
/// the selected item. The event loop reloads `items` whenever the filter
/// changes and reloads `detail` whenever `selected` changes.
#[derive(Default)]
pub struct MemoriesTabState {
    pub items: Vec<vestige_core::MemoryCard>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub filter_text: String,
    pub filter_focused: bool,
    pub detail: Option<vestige_core::MemoryDetail>,
    pub load_error: Option<String>,
}

impl MemoriesTabState {
    pub fn selected_id(&self) -> Option<&vestige_core::MemoryId> {
        self.items.get(self.selected).map(|c| &c.id)
    }

    /// Move the cursor by `delta` rows, clamping to `[0, len-1]`. Returns
    /// `true` if the cursor changed — the event loop uses this to decide
    /// whether to re-fetch detail.
    pub fn move_cursor(&mut self, delta: i64) -> bool {
        if self.items.is_empty() {
            self.selected = 0;
            return false;
        }
        let last = self.items.len().saturating_sub(1) as i64;
        let new = (self.selected as i64 + delta).clamp(0, last) as usize;
        if new == self.selected {
            return false;
        }
        self.selected = new;
        true
    }

    pub fn move_to(&mut self, target: usize) -> bool {
        if self.items.is_empty() {
            self.selected = 0;
            return false;
        }
        let new = target.min(self.items.len() - 1);
        if new == self.selected {
            return false;
        }
        self.selected = new;
        true
    }
}

/// The browser's full mutable state.
pub struct App {
    pub project_name: String,
    pub tab: Tab,
    pub counts: Counts,
    pub help_open: bool,
    pub should_quit: bool,
    pub memories: MemoriesTabState,
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
            memories: MemoriesTabState::default(),
        }
    }

    /// Pure-state actions. List navigation lives here; I/O-bearing actions
    /// (filter edits that trigger a reload, cursor moves that re-fetch detail)
    /// are interpreted by the event loop in `mod.rs` because they need a
    /// `&Store` handle.
    pub fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::NextTab => self.tab = self.tab.next(),
            Action::PrevTab => self.tab = self.tab.prev(),
            Action::ToggleHelp => self.help_open = !self.help_open,
            Action::CloseOverlay => {
                if self.help_open {
                    self.help_open = false;
                } else if self.tab == Tab::Memories && self.memories.filter_focused {
                    self.memories.filter_focused = false;
                }
            }
            Action::None => {}
            // Memories-tab navigation handled inline; the caller is responsible
            // for noticing the selection changed and re-fetching detail.
            Action::MoveDown
            | Action::MoveUp
            | Action::MoveTop
            | Action::MoveBottom
            | Action::HalfPageDown
            | Action::HalfPageUp
            | Action::OpenFilter
            | Action::FilterChar(_)
            | Action::FilterBackspace => {}
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

    fn card(label: &str) -> vestige_core::MemoryCard {
        use time::OffsetDateTime;
        use vestige_core::{MemoryId, MemoryStatus, MemoryType, RepresentationDepth};
        vestige_core::MemoryCard {
            id: MemoryId::new(),
            r#type: MemoryType::Note,
            status: MemoryStatus::Active,
            title: format!("{label} title"),
            one_liner: format!("{label} one-liner"),
            importance: 0.5,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            available_depths: vec![RepresentationDepth::OneLiner],
        }
    }

    fn populated_state(n: usize) -> MemoriesTabState {
        MemoriesTabState {
            items: (0..n).map(|i| card(&format!("item-{i}"))).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn move_cursor_clamps_at_bounds() {
        let mut s = populated_state(5);
        assert!(!s.move_cursor(-1));
        assert_eq!(s.selected, 0);
        assert!(s.move_cursor(2));
        assert_eq!(s.selected, 2);
        assert!(s.move_cursor(10));
        assert_eq!(s.selected, 4);
        assert!(!s.move_cursor(1));
    }

    #[test]
    fn move_cursor_on_empty_is_noop() {
        let mut s = MemoriesTabState::default();
        assert!(!s.move_cursor(5));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn move_to_jumps_or_clamps() {
        let mut s = populated_state(5);
        assert!(s.move_to(3));
        assert_eq!(s.selected, 3);
        assert!(s.move_to(99));
        assert_eq!(s.selected, 4);
    }

    #[test]
    fn close_overlay_clears_filter_focus_when_help_closed() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.memories.filter_focused = true;
        a.handle(Action::CloseOverlay);
        assert!(!a.memories.filter_focused);
    }

    #[test]
    fn close_overlay_prefers_help_over_filter() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.help_open = true;
        a.memories.filter_focused = true;
        a.handle(Action::CloseOverlay);
        assert!(!a.help_open, "help should close first");
        assert!(
            a.memories.filter_focused,
            "filter focus survives until help is closed"
        );
    }
}
