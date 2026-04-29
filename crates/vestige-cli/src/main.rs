use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;

#[derive(Parser)]
#[command(name = "vestige", version, about = "Repo-pinned memory layer for coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialise Vestige memory in the current repo.
    Init(commands::init::InitArgs),
    /// Show current Vestige project state.
    Status,
    /// Start the MCP server (M5 — not implemented yet).
    Mcp(commands::mcp::McpArgs),
}

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
        Command::Mcp(args) => commands::mcp::run(args),
    }
}
