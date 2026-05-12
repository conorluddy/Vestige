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
    // Provenance sub-views for the selected memory. Each loads its data
    // lazily on first request and stays cached until the cursor moves.
    ShowWhy,
    ShowSources,
    ShowTracesOf,
    // Mutations. `RequestForget` / `RequestRestore` open a confirmation modal.
    // `ConfirmYes` / `ConfirmNo` resolve it. The modal blocks everything else.
    RequestForget,
    RequestRestore,
    // Candidate mutations.
    RequestApprove,
    RequestReject,
    // Trace replay.
    RequestReplay,
    ConfirmYes,
    ConfirmNo,
    // Text-input modal editing (reject reason prompt).
    PromptChar(char),
    PromptBackspace,
    PromptSubmit,
    // Command palette (`:`).
    OpenPalette,
    PaletteChar(char),
    PaletteBackspace,
    PaletteSubmit,
    None,
}

/// Which content occupies the detail pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailView {
    #[default]
    Default,
    Why,
    Sources,
    TracesOf,
}

/// A modal blocking input until the user confirms, submits, or cancels.
///
/// Two shapes:
/// - **Confirm**: y / n with no extra input.
/// - **Prompt**: free-form text input with a buffer; Enter submits, Esc cancels.
#[derive(Debug, Clone)]
pub enum Modal {
    ConfirmForget(vestige_core::MemoryId),
    ConfirmRestore(vestige_core::MemoryId),
    ConfirmApprove(vestige_core::CandidateId),
    PromptRejectReason {
        id: vestige_core::CandidateId,
        buffer: String,
    },
}

impl Modal {
    pub fn verb(&self) -> &'static str {
        match self {
            Modal::ConfirmForget(_) => "Forget",
            Modal::ConfirmRestore(_) => "Restore",
            Modal::ConfirmApprove(_) => "Approve",
            Modal::PromptRejectReason { .. } => "Reject",
        }
    }

    pub fn subject_id(&self) -> String {
        match self {
            Modal::ConfirmForget(id) | Modal::ConfirmRestore(id) => id.as_str().to_string(),
            Modal::ConfirmApprove(id) => id.as_str().to_string(),
            Modal::PromptRejectReason { id, .. } => id.as_str().to_string(),
        }
    }

    /// Is this a text-input prompt rather than a y/n confirm?
    pub fn is_prompt(&self) -> bool {
        matches!(self, Modal::PromptRejectReason { .. })
    }
}

/// Short-lived flash message shown in the status line after a mutation.
#[derive(Debug, Clone)]
pub struct StatusFlash {
    pub text: String,
    pub is_error: bool,
}

/// Command palette state — opened with `:`, typed into, submitted with Enter.
///
/// `error` carries the last parse/execute error (cleared on the next keystroke).
#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    pub buffer: String,
    pub error: Option<String>,
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
    pub detail: Option<vestige_core::FetchedMemory>,
    pub load_error: Option<String>,
    pub detail_view: DetailView,
    pub provenance: ProvenanceCache,
}

/// Lazily-populated provenance data for the currently selected memory.
/// Cleared whenever the cursor moves so we never display stale data.
#[derive(Default)]
pub struct ProvenanceCache {
    pub events: Option<Vec<vestige_store::ProvenanceEvent>>,
    pub sources: Option<Vec<vestige_store::SourceReceiptRow>>,
    pub traces_of: Option<Vec<vestige_store::QueryEventRow>>,
}

impl ProvenanceCache {
    pub fn clear(&mut self) {
        self.events = None;
        self.sources = None;
        self.traces_of = None;
    }
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

/// Per-tab state for the Candidates tab. Same shape as memories but the row
/// type is `Candidate` and mutations are approve / reject.
#[derive(Default)]
pub struct CandidatesTabState {
    pub items: Vec<vestige_core::Candidate>,
    pub selected: usize,
    pub filter_text: String,
    pub filter_focused: bool,
    pub detail: Option<vestige_core::Candidate>,
    pub load_error: Option<String>,
    pub detail_view: DetailView,
    pub provenance: ProvenanceCache,
}

impl CandidatesTabState {
    pub fn selected_id(&self) -> Option<&vestige_core::CandidateId> {
        self.items.get(self.selected).map(|c| &c.id)
    }

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

/// Per-tab state for the Traces tab. Row type is `TraceCard`; detail loads
/// the full `TraceDetail`. Mutations are not supported (traces are
/// append-only audit) — only `p` replay, which surfaces a diff in a
/// dedicated `ReplayResult` cache.
#[derive(Default)]
pub struct TracesTabState {
    pub items: Vec<vestige_engine::TraceCard>,
    pub selected: usize,
    pub detail: Option<vestige_engine::TraceDetail>,
    pub load_error: Option<String>,
    pub replay: Option<vestige_engine::ReplayResult>,
}

impl TracesTabState {
    pub fn selected_id(&self) -> Option<&str> {
        self.items.get(self.selected).map(|c| c.trace_id.as_str())
    }

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
    pub candidates: CandidatesTabState,
    pub traces: TracesTabState,
    pub modal: Option<Modal>,
    pub status_flash: Option<StatusFlash>,
    pub palette: Option<CommandPalette>,
    /// Optional kind filter for the Memories tab.
    pub memories_kind_filter: Option<vestige_core::MemoryType>,
    /// Optional status filter for the Memories tab. `None` = active+deleted.
    pub memories_status_filter: Option<vestige_core::MemoryStatus>,
    /// Optional caller filter for the Traces tab.
    pub traces_caller_filter: Option<String>,
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
            candidates: CandidatesTabState::default(),
            traces: TracesTabState::default(),
            modal: None,
            status_flash: None,
            palette: None,
            memories_kind_filter: None,
            memories_status_filter: None,
            traces_caller_filter: None,
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
                // Precedence: modal > palette > help > filter > sub-view.
                if self.modal.is_some() {
                    self.modal = None;
                } else if self.palette.is_some() {
                    self.palette = None;
                } else if self.help_open {
                    self.help_open = false;
                } else if self.tab == Tab::Memories && self.memories.filter_focused {
                    self.memories.filter_focused = false;
                } else if self.tab == Tab::Candidates && self.candidates.filter_focused {
                    self.candidates.filter_focused = false;
                } else if self.tab == Tab::Memories
                    && self.memories.detail_view != DetailView::Default
                {
                    self.memories.detail_view = DetailView::Default;
                } else if self.tab == Tab::Candidates
                    && self.candidates.detail_view != DetailView::Default
                {
                    self.candidates.detail_view = DetailView::Default;
                }
            }
            Action::ConfirmNo => {
                self.modal = None;
            }
            Action::PromptChar(c) => {
                if let Some(Modal::PromptRejectReason { buffer, .. }) = &mut self.modal {
                    buffer.push(c);
                }
            }
            Action::PromptBackspace => {
                if let Some(Modal::PromptRejectReason { buffer, .. }) = &mut self.modal {
                    buffer.pop();
                }
            }
            Action::None => {}
            // Handled inline in the dispatcher because they need a `&Store`.
            Action::MoveDown
            | Action::MoveUp
            | Action::MoveTop
            | Action::MoveBottom
            | Action::HalfPageDown
            | Action::HalfPageUp
            | Action::OpenFilter
            | Action::FilterChar(_)
            | Action::FilterBackspace
            | Action::ShowWhy
            | Action::ShowSources
            | Action::ShowTracesOf
            | Action::RequestForget
            | Action::RequestRestore
            | Action::RequestApprove
            | Action::RequestReject
            | Action::RequestReplay
            | Action::ConfirmYes
            | Action::PromptSubmit
            | Action::OpenPalette
            | Action::PaletteChar(_)
            | Action::PaletteBackspace
            | Action::PaletteSubmit => {}
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
    fn close_overlay_returns_subview_to_default() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.memories.detail_view = DetailView::Why;
        a.handle(Action::CloseOverlay);
        assert_eq!(a.memories.detail_view, DetailView::Default);
    }

    #[test]
    fn close_overlay_prefers_filter_focus_over_subview() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.memories.filter_focused = true;
        a.memories.detail_view = DetailView::Sources;
        a.handle(Action::CloseOverlay);
        assert!(
            !a.memories.filter_focused,
            "filter focus should close first"
        );
        assert_eq!(
            a.memories.detail_view,
            DetailView::Sources,
            "subview is untouched until filter focus is closed"
        );
    }

    #[test]
    fn close_overlay_dismisses_modal_first() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.help_open = true;
        a.memories.detail_view = DetailView::Why;
        a.modal = Some(Modal::ConfirmForget(vestige_core::MemoryId::new()));
        a.handle(Action::CloseOverlay);
        assert!(a.modal.is_none(), "modal closes first");
        assert!(a.help_open, "help untouched until modal is closed");
        assert_eq!(a.memories.detail_view, DetailView::Why, "subview untouched");
    }

    #[test]
    fn close_overlay_dismisses_palette_after_modal() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.help_open = true;
        a.palette = Some(CommandPalette::default());
        a.handle(Action::CloseOverlay);
        assert!(a.palette.is_none(), "palette closes before help");
        assert!(a.help_open, "help still open");
        a.handle(Action::CloseOverlay);
        assert!(!a.help_open, "help closes second");
    }

    #[test]
    fn confirm_no_clears_modal() {
        let mut a = App::new(Tab::Memories, Counts::default(), "p".into());
        a.modal = Some(Modal::ConfirmRestore(vestige_core::MemoryId::new()));
        a.handle(Action::ConfirmNo);
        assert!(a.modal.is_none());
    }

    #[test]
    fn provenance_cache_clear_resets_all_three() {
        let mut cache = ProvenanceCache {
            events: Some(Vec::new()),
            sources: Some(Vec::new()),
            traces_of: Some(Vec::new()),
        };
        cache.clear();
        assert!(cache.events.is_none());
        assert!(cache.sources.is_none());
        assert!(cache.traces_of.is_none());
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
