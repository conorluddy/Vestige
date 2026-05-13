//! `vestige browse` — interactive TUI over project memory.
//!
//! Three tabs (Memories · Candidates · Traces) over a single Store handle held
//! for the session lifetime. Vim navigation, `/` lexical filter, `:` command
//! palette, provenance sub-views (`w` why · `s` sources · `t` traces-of),
//! mutations (`f` forget · `r` restore · `a` approve · `R` reject), and `p`
//! trace replay with inline diff. No daemon, no schema changes, no MCP changes.

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Args, ValueEnum};
use ratatui::crossterm::event::{self};

use vestige_config::discover_config;
use vestige_core::{resolve_default_mode, ProjectId, SearchMode};
use vestige_embed::EmbeddingProvider;
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

    // Resolve the initial search mode from config. If the configured default
    // requires a provider that is unavailable, fall back silently to Lexical —
    // the status line will show `mode:hybrid→lexical` to explain the fallback.
    let config_default = cfg.search.as_ref().and_then(|s| s.default_mode.as_deref());
    let requested_mode = resolve_default_mode(None, config_default).unwrap_or(SearchMode::Lexical);

    let counts = read_counts(&store, &project_id)?;
    let mut app = App::with_mode(
        args.tab.into(),
        counts,
        cfg.project_name.clone(),
        requested_mode,
    );

    // Lazy embedding provider — constructed on first semantic/hybrid use.
    // We build the context temporarily just for provider config resolution.
    let mut session_provider: Option<Box<dyn EmbeddingProvider>> = None;
    let embed_cfg = vestige_config::embeddings_config_for(cfg.embeddings.as_ref());

    let mut store = store;
    // Load all three tabs eagerly so the first frame has data on any tab.
    tabs::memories::reload_list(&mut app, &store, &project_id, session_provider.as_deref())?;
    tabs::candidates::reload_list(&mut app, &store, &project_id)?;
    tabs::traces::reload_list(&mut app, &store, &project_id)?;

    terminal::install_panic_hook();
    let mut term = terminal::enter().context("entering raw mode")?;
    let loop_result = run_loop(
        &mut term,
        &mut app,
        &mut store,
        &project_id,
        &mut session_provider,
        &embed_cfg,
    );
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
    session_provider: &mut Option<Box<dyn EmbeddingProvider>>,
    embed_cfg: &vestige_embed::EmbeddingsConfig,
) -> Result<()> {
    // Poll cadence: 250ms keeps Ctrl-c responsive while staying idle most of
    // the time. Frames only change on input.
    let poll = Duration::from_millis(250);
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(poll)? {
            let evt = event::read()?;
            let action = input::map_event(&evt, app);
            apply_action(app, store, project_id, session_provider, embed_cfg, action)?;
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
    session_provider: &mut Option<Box<dyn EmbeddingProvider>>,
    embed_cfg: &vestige_embed::EmbeddingsConfig,
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
        Action::MoveDown => move_active_tab(app, store, project_id, 1, false)?,
        Action::MoveUp => move_active_tab(app, store, project_id, -1, false)?,
        Action::MoveTop => move_active_tab(app, store, project_id, 0, true)?,
        Action::MoveBottom => move_active_tab(app, store, project_id, i64::MAX, true)?,
        Action::HalfPageDown => move_active_tab(app, store, project_id, 10, false)?,
        Action::HalfPageUp => move_active_tab(app, store, project_id, -10, false)?,
        Action::ShowWhy => match app.tab {
            Tab::Memories => {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::Why)?;
            }
            Tab::Candidates => {
                tabs::candidates::ensure_provenance(app, store, DetailView::Why)?;
            }
            Tab::Traces => {}
        },
        Action::ShowSources => match app.tab {
            Tab::Memories => {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::Sources)?;
            }
            Tab::Candidates => {
                tabs::candidates::ensure_provenance(app, store, DetailView::Sources)?;
            }
            Tab::Traces => {}
        },
        Action::ShowTracesOf => {
            if app.tab == Tab::Memories {
                tabs::memories::ensure_provenance(app, store, project_id, DetailView::TracesOf)?;
            }
        }
        Action::OpenFilter => match app.tab {
            Tab::Memories => app.memories.filter_focused = true,
            Tab::Candidates => app.candidates.filter_focused = true,
            Tab::Traces => {}
        },
        Action::FilterChar(c) => match app.tab {
            Tab::Memories => {
                app.memories.filter_text.push(c);
                ensure_provider_for_mode(app, session_provider, embed_cfg);
                tabs::memories::reload_list(app, store, project_id, session_provider.as_deref())?;
            }
            Tab::Candidates => {
                app.candidates.filter_text.push(c);
                tabs::candidates::reload_list(app, store, project_id)?;
            }
            Tab::Traces => {}
        },
        Action::FilterBackspace => match app.tab {
            Tab::Memories => {
                if app.memories.filter_text.pop().is_some() {
                    ensure_provider_for_mode(app, session_provider, embed_cfg);
                    tabs::memories::reload_list(
                        app,
                        store,
                        project_id,
                        session_provider.as_deref(),
                    )?;
                } else {
                    app.memories.filter_focused = false;
                }
            }
            Tab::Candidates => {
                if app.candidates.filter_text.pop().is_some() {
                    tabs::candidates::reload_list(app, store, project_id)?;
                } else {
                    app.candidates.filter_focused = false;
                }
            }
            Tab::Traces => {}
        },
        Action::RequestForget => {
            if app.tab == Tab::Memories {
                if let Some(card) = app.memories.items.get(app.memories.selected) {
                    if card.status == vestige_core::MemoryStatus::Active {
                        app.modal = Some(app::Modal::ConfirmForget(card.id.clone()));
                    }
                }
            }
        }
        Action::RequestRestore => {
            if app.tab == Tab::Memories {
                if let Some(card) = app.memories.items.get(app.memories.selected) {
                    if card.status == vestige_core::MemoryStatus::Deleted {
                        app.modal = Some(app::Modal::ConfirmRestore(card.id.clone()));
                    }
                }
            }
        }
        Action::RequestApprove => {
            if app.tab == Tab::Candidates {
                if let Some(id) = app.candidates.selected_id().cloned() {
                    app.modal = Some(app::Modal::ConfirmApprove(id));
                }
            }
        }
        Action::RequestReject => {
            if app.tab == Tab::Candidates {
                if let Some(id) = app.candidates.selected_id().cloned() {
                    app.modal = Some(app::Modal::PromptRejectReason {
                        id,
                        buffer: String::new(),
                    });
                }
            }
        }
        Action::PromptChar(_) | Action::PromptBackspace => {
            app.handle(action);
        }
        Action::PromptSubmit => {
            apply_prompt_submit(app, store, project_id)?;
        }
        Action::ConfirmYes => {
            apply_confirmed_mutation(app, store, project_id)?;
        }
        Action::OpenPalette => {
            app.palette = Some(app::CommandPalette::default());
        }
        Action::PaletteChar(c) => {
            if let Some(p) = &mut app.palette {
                p.buffer.push(c);
                p.error = None;
            }
        }
        Action::PaletteBackspace => {
            if let Some(p) = &mut app.palette {
                p.buffer.pop();
                p.error = None;
            }
        }
        Action::PaletteSubmit => {
            execute_palette(app, store, project_id, session_provider, embed_cfg)?;
        }
        Action::RequestReplay => {
            if app.tab == Tab::Traces {
                ensure_provider_for_mode(app, session_provider, embed_cfg);
                tabs::traces::replay_selected(app, store, project_id, session_provider.as_deref())?;
                if let Some(replay) = &app.traces.replay {
                    let added = replay.diff.added.len();
                    let removed = replay.diff.removed.len();
                    app.status_flash = Some(app::StatusFlash {
                        text: format!(
                            "Replayed {} → +{added} −{removed} (new {})",
                            replay.trace_id, replay.replay_trace_id
                        ),
                        is_error: !replay.provider_match || replay.mode_fallback,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Resolve a binary confirm by mutating the store, reloading the affected
/// list at the same cursor index, and flashing the outcome.
fn apply_confirmed_mutation(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
) -> Result<()> {
    let Some(modal) = app.modal.take() else {
        return Ok(());
    };
    let outcome: std::result::Result<bool, anyhow::Error> = match &modal {
        app::Modal::ConfirmForget(id) => store.forget_memory(id).map_err(Into::into),
        app::Modal::ConfirmRestore(id) => store.restore_memory(id).map_err(Into::into),
        app::Modal::ConfirmApprove(id) => {
            let result = vestige_engine::approve_candidate(
                store,
                project_id,
                id,
                vestige_engine::ApprovalOverrides::default(),
            );
            match result {
                Ok(out) => {
                    app.status_flash = Some(app::StatusFlash {
                        text: format!("Approved {} → {}", out.candidate_id, out.memory_id),
                        is_error: false,
                    });
                    let prev = app.candidates.selected;
                    tabs::candidates::reload_list(app, store, project_id)?;
                    if prev < app.candidates.items.len() {
                        app.candidates.selected = prev;
                        tabs::candidates::refresh_detail(app, store)?;
                    }
                    return Ok(());
                }
                Err(e) => Err(e.into()),
            }
        }
        app::Modal::PromptRejectReason { .. } => {
            // Prompt modals resolve through PromptSubmit, not ConfirmYes. If we
            // get here the input mapper has drifted — surface in debug builds,
            // restore the modal in release so a user never loses their typed
            // buffer to a swallow.
            debug_assert!(false, "ConfirmYes on prompt modal");
            app.modal = Some(modal);
            return Ok(());
        }
    };
    let subject = modal.subject_id();
    match outcome {
        Ok(true) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} {}", past_tense(&modal), subject),
                is_error: false,
            });
        }
        Ok(false) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} skipped (no-op) for {}", modal.verb(), subject),
                is_error: true,
            });
        }
        Err(e) => {
            app.status_flash = Some(app::StatusFlash {
                text: format!("{} failed: {e}", modal.verb()),
                is_error: true,
            });
        }
    }
    // Reload whichever tab was affected. Both reloads are cheap.
    // Forget/restore change the active list so we need to reload. Use lexical
    // only here (mutations don't carry a search context).
    if matches!(
        modal,
        app::Modal::ConfirmForget(_) | app::Modal::ConfirmRestore(_)
    ) {
        let prev = app.memories.selected;
        tabs::memories::reload_list(app, store, project_id, None)?;
        if prev < app.memories.items.len() {
            app.memories.selected = prev;
            tabs::memories::refresh_detail(app, store)?;
        }
    }
    Ok(())
}

/// Resolve a text-input prompt by submitting whatever's in the buffer.
///
/// Rejections must carry a reason (PRD §16 — rejects are reasoned and final).
/// An empty buffer re-opens the prompt with a status-flash hint rather than
/// silently rejecting with `Other("unspecified")`.
fn apply_prompt_submit(app: &mut App, store: &mut Store, project_id: &ProjectId) -> Result<()> {
    let Some(modal) = app.modal.take() else {
        return Ok(());
    };
    match modal {
        app::Modal::PromptRejectReason { id, buffer } => {
            if buffer.trim().is_empty() {
                app.modal = Some(app::Modal::PromptRejectReason { id, buffer });
                app.status_flash = Some(app::StatusFlash {
                    text: "Reject needs a reason — type one of: duplicate / wrong / not_durable / too_noisy / stale, or free text. Esc cancels.".into(),
                    is_error: true,
                });
                return Ok(());
            }
            let reason = parse_reject_reason(&buffer);
            let result = vestige_engine::reject_candidate(
                store,
                project_id,
                &id,
                reason.clone(),
                None,
                None,
            );
            match result {
                Ok(()) => {
                    app.status_flash = Some(app::StatusFlash {
                        text: format!("Rejected {} (reason: {reason})", id),
                        is_error: false,
                    });
                    let prev = app.candidates.selected;
                    tabs::candidates::reload_list(app, store, project_id)?;
                    if prev < app.candidates.items.len() {
                        app.candidates.selected = prev;
                        tabs::candidates::refresh_detail(app, store)?;
                    }
                }
                Err(e) => {
                    app.status_flash = Some(app::StatusFlash {
                        text: format!("Reject failed: {e}"),
                        is_error: true,
                    });
                }
            }
        }
        other => {
            // Wrong action for this modal type — restore it.
            app.modal = Some(other);
        }
    }
    Ok(())
}

/// Parse the reject prompt buffer into a typed `RejectionReason`. The caller
/// guarantees the input is non-empty (empty buffers are gated in
/// `apply_prompt_submit`); recognised tokens parse into typed variants, anything
/// else passes through as `Other`.
fn parse_reject_reason(input: &str) -> vestige_core::RejectionReason {
    use std::str::FromStr;
    let trimmed = input.trim();
    vestige_core::RejectionReason::from_str(trimmed)
        .unwrap_or_else(|_| vestige_core::RejectionReason::Other(trimmed.to_string()))
}

/// Lazily initialise the embedding provider when the current session mode
/// requires one. On failure, silently falls back to `Lexical` and records
/// `mode_fallback_from` so the status line can show `mode:hybrid→lexical`.
fn ensure_provider_for_mode(
    app: &mut App,
    session_provider: &mut Option<Box<dyn EmbeddingProvider>>,
    embed_cfg: &vestige_embed::EmbeddingsConfig,
) {
    if !matches!(app.search_mode, SearchMode::Semantic | SearchMode::Hybrid) {
        return;
    }
    if session_provider.is_some() {
        return;
    }
    match vestige_embed::build_provider(embed_cfg) {
        Ok(p) => {
            *session_provider = Some(p);
            app.mode_fallback_from = None;
        }
        Err(_) => {
            // Provider unavailable — fall back to lexical for this operation.
            let requested = app.search_mode;
            app.search_mode = SearchMode::Lexical;
            app.mode_fallback_from = Some(requested);
        }
    }
}

/// Produce the status-line mode suffix. Returns `None` when the mode is lexical
/// and no fallback is recorded (the common case — keeps the line clean).
pub(crate) fn mode_display_label(mode: SearchMode, fallback_from: Option<SearchMode>) -> String {
    if let Some(requested) = fallback_from {
        return format!("{}→lexical", mode_name(requested));
    }
    mode_name(mode).to_string()
}

fn mode_name(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Lexical => "lexical",
        SearchMode::Semantic => "semantic",
        SearchMode::Hybrid => "hybrid",
    }
}

fn execute_palette(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    session_provider: &mut Option<Box<dyn EmbeddingProvider>>,
    embed_cfg: &vestige_embed::EmbeddingsConfig,
) -> Result<()> {
    let raw = match app.palette.as_ref() {
        Some(p) => p.buffer.trim().to_string(),
        None => return Ok(()),
    };
    if raw.is_empty() {
        app.palette = None;
        return Ok(());
    }
    let (cmd, rest) = match raw.split_once(' ') {
        Some((c, r)) => (c.to_string(), r.trim().to_string()),
        None => (raw.clone(), String::new()),
    };
    let result: std::result::Result<Option<String>, String> = match cmd.as_str() {
        "q" | "quit" => {
            app.should_quit = true;
            Ok(None)
        }
        "help" => {
            app.help_open = true;
            Ok(None)
        }
        "goto" => execute_goto(app, store, project_id, &rest),
        "kind" => execute_kind(app, store, project_id, session_provider.as_deref(), &rest),
        "status" => execute_status(app, store, project_id, session_provider.as_deref(), &rest),
        "caller" => execute_caller(app, store, project_id, &rest),
        "search" => execute_search_cmd(app, store, project_id, session_provider.as_deref(), &rest),
        "mode" => execute_mode(app, store, project_id, session_provider, embed_cfg, &rest),
        other => Err(format!("unknown command: {other}")),
    };
    match result {
        Ok(flash) => {
            app.palette = None;
            if let Some(text) = flash {
                app.status_flash = Some(app::StatusFlash {
                    text,
                    is_error: false,
                });
            }
        }
        Err(msg) => {
            if let Some(p) = &mut app.palette {
                p.error = Some(msg);
            }
        }
    }
    Ok(())
}

/// Handle `:mode lexical|semantic|hybrid`. Validates the value, initialises the
/// provider lazily if needed, updates `app.search_mode`, and reloads the
/// Memories list so the new mode takes effect immediately.
fn execute_mode(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    session_provider: &mut Option<Box<dyn EmbeddingProvider>>,
    embed_cfg: &vestige_embed::EmbeddingsConfig,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    let mode = match arg {
        "lexical" => SearchMode::Lexical,
        "semantic" => SearchMode::Semantic,
        "hybrid" => SearchMode::Hybrid,
        "" => {
            let current = mode_display_label(app.search_mode, app.mode_fallback_from);
            return Ok(Some(format!("current mode: {current}")));
        }
        other => {
            return Err(format!(
                "mode must be lexical | semantic | hybrid; got {other}"
            ))
        }
    };
    // Try to build a provider if the mode needs one.
    if matches!(mode, SearchMode::Semantic | SearchMode::Hybrid) {
        if session_provider.is_none() {
            match vestige_embed::build_provider(embed_cfg) {
                Ok(p) => {
                    *session_provider = Some(p);
                    app.mode_fallback_from = None;
                }
                Err(e) => {
                    let hint = match &e {
                        vestige_embed::EmbedError::ProviderDisabled(name) => {
                            format!(
                                "provider `{name}` not compiled — rebuild with `--features {name}`"
                            )
                        }
                        _ => e.to_string(),
                    };
                    // Fall back to lexical and surface in status line.
                    app.search_mode = SearchMode::Lexical;
                    app.mode_fallback_from = Some(mode);
                    tabs::memories::reload_list(app, store, project_id, None)
                        .map_err(|e| e.to_string())?;
                    return Ok(Some(format!("mode fallback → lexical ({hint})")));
                }
            }
        }
    } else {
        // Switching to lexical — clear any fallback state.
        app.mode_fallback_from = None;
    }
    app.search_mode = mode;
    tabs::memories::reload_list(app, store, project_id, session_provider.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(Some(format!("mode set to {arg}")))
}

fn execute_goto(
    app: &mut App,
    store: &Store,
    project_id: &ProjectId,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    let id = arg.trim();
    if id.is_empty() {
        return Err("usage: goto <mem_…|cand_…|trace_…>".into());
    }
    if id.starts_with("mem_") {
        if let Some(pos) = app.memories.items.iter().position(|c| c.id.as_str() == id) {
            app.tab = Tab::Memories;
            app.memories.selected = pos;
            tabs::memories::refresh_detail(app, store).map_err(|e| e.to_string())?;
            return Ok(Some(format!("→ {id}")));
        }
        return Err(format!("no memory in current list with id {id}"));
    }
    if id.starts_with("cand_") {
        if let Some(pos) = app
            .candidates
            .items
            .iter()
            .position(|c| c.id.as_str() == id)
        {
            app.tab = Tab::Candidates;
            app.candidates.selected = pos;
            tabs::candidates::refresh_detail(app, store).map_err(|e| e.to_string())?;
            return Ok(Some(format!("→ {id}")));
        }
        return Err(format!("no candidate in current list with id {id}"));
    }
    if id.starts_with("trace_") {
        if let Some(pos) = app.traces.items.iter().position(|c| c.trace_id == id) {
            app.tab = Tab::Traces;
            app.traces.selected = pos;
            tabs::traces::refresh_detail(app, store, project_id).map_err(|e| e.to_string())?;
            return Ok(Some(format!("→ {id}")));
        }
        return Err(format!("no trace in current list with id {id}"));
    }
    Err(format!(
        "id must start with mem_, cand_, or trace_; got {id}"
    ))
}

fn execute_kind(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    provider: Option<&dyn EmbeddingProvider>,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    use std::str::FromStr;
    if app.tab != Tab::Memories {
        return Err(":kind only applies to the Memories tab".into());
    }
    if arg.is_empty() || arg == "all" {
        app.memories_kind_filter = None;
        tabs::memories::reload_list(app, store, project_id, provider).map_err(|e| e.to_string())?;
        return Ok(Some("kind: all".into()));
    }
    let kind = vestige_core::MemoryType::from_str(arg)
        .map_err(|_| format!("unknown memory type: {arg}"))?;
    app.memories_kind_filter = Some(kind);
    tabs::memories::reload_list(app, store, project_id, provider).map_err(|e| e.to_string())?;
    Ok(Some(format!("kind: {arg}")))
}

fn execute_status(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    provider: Option<&dyn EmbeddingProvider>,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    if app.tab != Tab::Memories {
        return Err(":status only applies to the Memories tab".into());
    }
    app.memories_status_filter = match arg {
        "" | "all" => None,
        "active" => Some(vestige_core::MemoryStatus::Active),
        "deleted" => Some(vestige_core::MemoryStatus::Deleted),
        other => {
            return Err(format!(
                "status must be active | deleted | all; got {other}"
            ))
        }
    };
    tabs::memories::reload_list(app, store, project_id, provider).map_err(|e| e.to_string())?;
    Ok(Some(format!(
        "status: {}",
        if arg.is_empty() { "all" } else { arg }
    )))
}

fn execute_caller(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    if app.tab != Tab::Traces {
        return Err(":caller only applies to the Traces tab".into());
    }
    app.traces_caller_filter = match arg {
        "" | "all" => None,
        "cli" | "mcp" => Some(arg.to_string()),
        other => return Err(format!("caller must be cli | mcp | all; got {other}")),
    };
    tabs::traces::reload_list(app, store, project_id).map_err(|e| e.to_string())?;
    Ok(Some(format!(
        "caller: {}",
        if arg.is_empty() { "all" } else { arg }
    )))
}

fn execute_search_cmd(
    app: &mut App,
    store: &mut Store,
    project_id: &ProjectId,
    provider: Option<&dyn EmbeddingProvider>,
    arg: &str,
) -> std::result::Result<Option<String>, String> {
    let text = arg.trim().to_string();
    match app.tab {
        Tab::Memories => {
            app.memories.filter_text = text.clone();
            tabs::memories::reload_list(app, store, project_id, provider)
                .map_err(|e| e.to_string())?;
        }
        Tab::Candidates => {
            app.candidates.filter_text = text.clone();
            tabs::candidates::reload_list(app, store, project_id).map_err(|e| e.to_string())?;
        }
        Tab::Traces => return Err(":search not supported on Traces yet".into()),
    }
    Ok(Some(format!("search: {text}")))
}

fn past_tense(modal: &app::Modal) -> &'static str {
    match modal {
        app::Modal::ConfirmForget(_) => "Forgot",
        app::Modal::ConfirmRestore(_) => "Restored",
        app::Modal::ConfirmApprove(_) => "Approved",
        app::Modal::PromptRejectReason { .. } => "Rejected",
    }
}

/// Move the cursor on the active tab. `delta` is a relative offset; when
/// `absolute` is true, the value is interpreted as a target index (with
/// `i64::MAX` meaning "last row"). Clears cached provenance + refreshes the
/// detail row when the cursor moved.
fn move_active_tab(
    app: &mut App,
    store: &Store,
    project_id: &ProjectId,
    delta: i64,
    absolute: bool,
) -> Result<()> {
    let moved = match app.tab {
        Tab::Memories => {
            if absolute {
                let target = if delta == i64::MAX {
                    app.memories.items.len().saturating_sub(1)
                } else {
                    delta.max(0) as usize
                };
                let m = app.memories.move_to(target);
                if m {
                    app.memories.detail_view = DetailView::Default;
                    app.memories.provenance.clear();
                    tabs::memories::refresh_detail(app, store)?;
                }
                m
            } else {
                let m = app.memories.move_cursor(delta);
                if m {
                    app.memories.detail_view = DetailView::Default;
                    app.memories.provenance.clear();
                    tabs::memories::refresh_detail(app, store)?;
                }
                m
            }
        }
        Tab::Candidates => {
            if absolute {
                let target = if delta == i64::MAX {
                    app.candidates.items.len().saturating_sub(1)
                } else {
                    delta.max(0) as usize
                };
                let m = app.candidates.move_to(target);
                if m {
                    app.candidates.detail_view = DetailView::Default;
                    app.candidates.provenance.clear();
                    tabs::candidates::refresh_detail(app, store)?;
                }
                m
            } else {
                let m = app.candidates.move_cursor(delta);
                if m {
                    app.candidates.detail_view = DetailView::Default;
                    app.candidates.provenance.clear();
                    tabs::candidates::refresh_detail(app, store)?;
                }
                m
            }
        }
        Tab::Traces => {
            if absolute {
                let target = if delta == i64::MAX {
                    app.traces.items.len().saturating_sub(1)
                } else {
                    delta.max(0) as usize
                };
                let m = app.traces.move_to(target);
                if m {
                    tabs::traces::refresh_detail(app, store, project_id)?;
                }
                m
            } else {
                let m = app.traces.move_cursor(delta);
                if m {
                    tabs::traces::refresh_detail(app, store, project_id)?;
                }
                m
            }
        }
    };
    let _ = moved;
    Ok(())
}
