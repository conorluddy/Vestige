//! `vestige browse` — interactive TUI over project memory.
//!
//! M2 ships the Memories tab read-only: a two-pane list+detail view with vim
//! navigation, per-keystroke `/` filter, soft-deleted entries shown with
//! strike-through, and rich empty-state copy. Candidates and Traces tabs are
//! still placeholders — they land in M5 and M6.

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Args, ValueEnum};
use ratatui::crossterm::event::{self};

use vestige_config::discover_config;
use vestige_core::ProjectId;
use vestige_store::Store;

mod app;
mod input;
mod tabs;
mod terminal;
mod ui;

pub use app::Tab;

use app::{Action, App, Counts, DetailView};

// === PUBLIC API ===

#[derive(Args)]
pub struct BrowseArgs {
    /// Initial tab to focus.
    #[arg(long, value_enum, default_value_t = TabArg::Memories)]
    pub tab: TabArg,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TabArg {
    Memories,
    Candidates,
    Traces,
}

impl From<TabArg> for Tab {
    fn from(value: TabArg) -> Self {
        match value {
            TabArg::Memories => Tab::Memories,
            TabArg::Candidates => Tab::Candidates,
            TabArg::Traces => Tab::Traces,
        }
    }
}

pub fn run(args: BrowseArgs) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(anyhow!(
            "`vestige browse` is interactive and needs a TTY on stdin and stdout — \
             pipe-friendly inspection lives in `list`, `show`, `search`, `why`, `sources`, `trace`."
        ));
    }
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (_config_path, cfg) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;
    let project_id = cfg.project_id()?;
    let storage_path = cfg.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;

    let counts = read_counts(&store, &project_id)?;
    let mut app = App::new(args.tab.into(), counts, cfg.project_name.clone());

    let mut store = store;
    // Load the Memories tab eagerly so the first frame has data.
    tabs::memories::reload_list(&mut app, &store, &project_id)?;

    terminal::install_panic_hook();
    let mut term = terminal::enter().context("entering raw mode")?;
    let loop_result = run_loop(&mut term, &mut app, &mut store, &project_id);
    let restore_result = terminal::leave(term);
    loop_result.and(restore_result)
}

// === PRIVATE ===

fn read_counts(store: &Store, project_id: &ProjectId) -> Result<Counts> {
    let memory = store.memory_counts(project_id)?;
    let candidates_pending = store.pending_candidate_count(project_id)?;
    let traces = store.query_event_count(project_id.as_str())? as i64;
    Ok(Counts {
        memories_active: memory.active,
        candidates_pending,
        traces,
    })
}

fn run_loop(
    terminal: &mut terminal::Tui,
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
) -> Result<()> {
    // Poll cadence: 250ms keeps Ctrl-c responsive while staying idle most of
    // the time. Frames only change on input.
    let poll = Duration::from_millis(250);
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(poll)? {
            let evt = event::read()?;
            let action = input::map_event(&evt, app);
            apply_action(app, store, project_id, action)?;
        }
    }
    Ok(())
}

/// Dispatch an action. Most actions are pure state changes routed through
/// `App::handle`; list-navigation actions trigger a detail re-fetch, and
/// filter edits trigger a list reload. The store lives here, not in `app`,
/// so the pure-state module stays I/O-free.
fn apply_action(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    action: Action,
) -> Result<()> {
    // A status flash sticks until the next non-trivial action arrives.
    // Polling timeouts (Action::None) don't count — they fire on the 250ms
    // poll cadence and would clear the flash before the user reads it.
    if !matches!(action, Action::None) {
        app.status_flash = None;
    }
    match action {
        Action::None => {}
        Action::Quit
        | Action::NextTab
        | Action::PrevTab
        | Action::ToggleHelp
        | Action::CloseOverlay
        | Action::ConfirmNo => {
            app.handle(action);
        }
        Action::MoveDown => move_and_refresh(app, store, |s| s.move_cursor(1))?,
        Action::MoveUp => move_and_refresh(app, store, |s| s.move_cursor(-1))?,
        Action::MoveTop => move_and_refresh(app, store, |s| s.move_to(0))?,
        Action::MoveBottom => {
            let last = app.memories.items.len().saturating_sub(1);
            move_and_refresh(app, store, |s| s.move_to(last))?;
        }
        Action::HalfPageDown => move_and_refresh(app, store, |s| s.move_cursor(10))?,
        Action::HalfPageUp => move_and_refresh(app, store, |s| s.move_cursor(-10))?,
        Action::ShowWhy => {
            if app.tab == Tab::Memories {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::Why)?;
            }
        }
        Action::ShowSources => {
            if app.tab == Tab::Memories {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::Sources)?;
            }
        }
        Action::ShowTracesOf => {
            if app.tab == Tab::Memories {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::TracesOf)?;
            }
        }
        Action::OpenFilter => {
            app.memories.filter_focused = true;
        }
        Action::FilterChar(c) => {
            app.memories.filter_text.push(c);
            tabs::memories::reload_list(app, store, project_id)?;
        }
        Action::FilterBackspace => {
            if app.memories.filter_text.pop().is_some() {
                tabs::memories::reload_list(app, store, project_id)?;
            } else {
                app.memories.filter_focused = false;
            }
        }
        Action::RequestForget => {
            if app.tab == Tab::Memories {
                request_mutation(app, |status, id| {
                    (status == vestige_core::MemoryStatus::Active)
                        .then_some(app::PendingConfirm::Forget(id))
                });
            }
        }
        Action::RequestRestore => {
            if app.tab == Tab::Memories {
                request_mutation(app, |status, id| {
                    (status == vestige_core::MemoryStatus::Deleted)
                        .then_some(app::PendingConfirm::Restore(id))
                });
            }
        }
        Action::ConfirmYes => {
            apply_confirmed_mutation(app, store, project_id)?;
        }
    }
    Ok(())
}

/// Open the mutation confirm modal if the current selection's status allows it.
fn request_mutation(
    app: &mut App,
    decide: impl FnOnce(
        vestige_core::MemoryStatus,
        vestige_core::MemoryId,
    ) -> Option<app::PendingConfirm>,
) {
    let Some(card) = app.memories.items.get(app.memories.selected) else {
        return;
    };
    let Some(pending) = decide(card.status, card.id.clone()) else {
        return;
    };
    app.pending_confirm = Some(pending);
}

/// Resolve the pending confirm by actually mutating the store, then reload
/// the list at the same cursor index. Stashes a [`StatusFlash`] message so
/// the user sees what just happened.
fn apply_confirmed_mutation(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
) -> Result<()> {
    let Some(pending) = app.pending_confirm.take() else {
        return Ok(());
    };
    let id = pending.memory_id().clone();
    let outcome = match &pending {
        app::PendingConfirm::Forget(_) => store.forget_memory(&id),
        app::PendingConfirm::Restore(_) => store.restore_memory(&id),
    };
    match outcome {
        Ok(true) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} {}", past_tense(&pending), id.as_str()),
                is_error: false,
            });
        }
        Ok(false) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} skipped (no-op) for {}", pending.verb(), id.as_str()),
                is_error: true,
            });
        }
        Err(e) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} failed: {e}", pending.verb()),
                is_error: true,
            });
        }
    }
    let prev_index = app.memories.selected;
    tabs::memories::reload_list(app, store, project_id)?;
    // Keep cursor in roughly the same place. `reload_list` already clamps
    // when the list shrinks; if the list grew, restore the prior index.
    if prev_index < app.memories.items.len() {
        app.memories.selected = prev_index;
        tabs::memories::refresh_detail(app, store)?;
    }
    Ok(())
}

fn past_tense(pending: &app::PendingConfirm) -> &'static str {
    match pending {
        app::PendingConfirm::Forget(_) => "Forgot",
        app::PendingConfirm::Restore(_) => "Restored",
    }
}

/// Move the cursor via `mutate`, and if the selection actually changed,
/// reset cached provenance + refresh the detail row.
fn move_and_refresh(
    app: &mut App,
    store: &Store,
    mutate: impl FnOnce(&mut app::MemoriesTabState) -> bool,
) -> Result<()> {
    if mutate(&mut app.memories) {
        app.memories.detail_view = DetailView::Default;
        app.memories.provenance.clear();
        tabs::memories::refresh_detail(app, store)?;
    }
    Ok(())
}
