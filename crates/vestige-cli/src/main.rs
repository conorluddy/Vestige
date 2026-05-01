//! `vestige` — repo-pinned memory layer for coding agents.
//!
//! Entry point for the `vestige` binary. Parses the top-level [`Command`] enum
//! with clap, initialises the `tracing` subscriber (stderr, `VESTIGE_LOG` env
//! filter, default level `warn`), then dispatches to the relevant command
//! handler. No business logic lives here.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;
mod context;
mod output;

/// Top-level clap entry-point.
#[derive(Parser)]
#[command(
    name = "vestige",
    version,
    about = "Repo-pinned memory layer for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// All top-level subcommands. Each variant's `///` comment becomes the clap `--help` line.
#[derive(Subcommand)]
enum Command {
    /// Initialise Vestige memory in the current repo.
    Init(commands::init::InitArgs),
    /// Show current Vestige project state.
    Status,
    /// Capture a free-form memory (default type: note).
    Remember(commands::remember::RememberArgs),
    /// Capture a note.
    Note(commands::note::NoteArgs),
    /// Capture a project decision.
    Decision(commands::decision::DecisionArgs),
    /// Capture a project preference.
    Preference(commands::preference::PreferenceArgs),
    /// Capture an open question.
    Question(commands::question::QuestionArgs),
    /// List active project memories.
    List(commands::list::ListArgs),
    /// Search project memory (FTS5 over all representations).
    Search(commands::search::SearchArgs),
    /// Recall — like search, opinionated for agent/user retrieval.
    Recall(commands::recall::RecallArgs),
    /// Render the project context pack (summary + decisions + questions + recent).
    Context(commands::context::ContextArgs),
    /// Show a memory at the given depth.
    Show(commands::show::ShowArgs),
    /// Soft-delete a memory.
    Forget(commands::forget::ForgetArgs),
    /// Restore a soft-deleted memory.
    Restore(commands::restore::RestoreArgs),
    /// Embed memory representations using an embedding provider.
    Embed(commands::embed::EmbedArgs),
    /// Manage embedding indexes (status, and future clear/stale).
    Embeddings(commands::embeddings::EmbeddingsArgs),
    /// Rebuild the FTS and/or embedding indexes.
    Reindex(commands::reindex::ReindexArgs),
    /// Start the MCP server over stdio so an agent can read/write project memory.
    Mcp(commands::mcp::McpArgs),
}

/// Initialise the tracing subscriber and dispatch to the resolved subcommand.
fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("VESTIGE_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => commands::init::run(args),
        Command::Status => commands::status::run(),
        Command::Remember(args) => commands::remember::run(args),
        Command::Note(args) => commands::note::run(args),
        Command::Decision(args) => commands::decision::run(args),
        Command::Preference(args) => commands::preference::run(args),
        Command::Question(args) => commands::question::run(args),
        Command::List(args) => commands::list::run(args),
        Command::Search(args) => commands::search::run(args),
        Command::Recall(args) => commands::recall::run(args),
        Command::Context(args) => commands::context::run(args),
        Command::Show(args) => commands::show::run(args),
        Command::Forget(args) => commands::forget::run(args),
        Command::Restore(args) => commands::restore::run(args),
        Command::Embed(args) => commands::embed::run(args),
        Command::Embeddings(args) => commands::embeddings::run(args),
        Command::Reindex(args) => commands::reindex::run(args),
        Command::Mcp(args) => commands::mcp::run(args),
    }
}
