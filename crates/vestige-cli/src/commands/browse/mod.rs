//! `vestige browse` — interactive TUI over project memory.
//!
//! M1 scaffolding only: launches a full-screen alt-screen UI with three tab
//! placeholders (Memories | Candidates | Traces), shows counts at startup, and
//! quits cleanly. List rendering, mutations, and provenance sub-views land in
//! M2 onwards.

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
mod terminal;
mod ui;

pub use app::Tab;

use app::{App, Counts};

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

    terminal::install_panic_hook();
    let mut term = terminal::enter().context("entering raw mode")?;
    let loop_result = run_loop(&mut term, &mut app);
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

fn run_loop(terminal: &mut terminal::Tui, app: &mut App) -> Result<()> {
    // Poll cadence: 250ms keeps Ctrl-c responsive while staying idle most of
    // the time. M1 doesn't need a render tick — frames only change on input.
    let poll = Duration::from_millis(250);
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(poll)? {
            let evt = event::read()?;
            let action = input::map_event(&evt, app);
            app.handle(action);
        }
    }
    Ok(())
}
