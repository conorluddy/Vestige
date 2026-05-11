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

use app::{Action, App, Counts};

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

    // Load the Memories tab eagerly so the first frame has data.
    tabs::memories::reload_list(&mut app, &store, &project_id)?;

    terminal::install_panic_hook();
    let mut term = terminal::enter().context("entering raw mode")?;
    let loop_result = run_loop(&mut term, &mut app, &store, &project_id);
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
    store: &Store,
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
    store: &Store,
    project_id: &ProjectId,
    action: Action,
) -> Result<()> {
    match action {
        Action::None => {}
        Action::Quit
        | Action::NextTab
        | Action::PrevTab
        | Action::ToggleHelp
        | Action::CloseOverlay => {
            app.handle(action);
        }
        Action::MoveDown => {
            if app.memories.move_cursor(1) {
                tabs::memories::refresh_detail(app, store)?;
            }
        }
        Action::MoveUp => {
            if app.memories.move_cursor(-1) {
                tabs::memories::refresh_detail(app, store)?;
            }
        }
        Action::MoveTop => {
            if app.memories.move_to(0) {
                tabs::memories::refresh_detail(app, store)?;
            }
        }
        Action::MoveBottom => {
            let last = app.memories.items.len().saturating_sub(1);
            if app.memories.move_to(last) {
                tabs::memories::refresh_detail(app, store)?;
            }
        }
        Action::HalfPageDown => {
            if app.memories.move_cursor(10) {
                tabs::memories::refresh_detail(app, store)?;
            }
        }
        Action::HalfPageUp => {
            if app.memories.move_cursor(-10) {
                tabs::memories::refresh_detail(app, store)?;
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
    }
    Ok(())
}
