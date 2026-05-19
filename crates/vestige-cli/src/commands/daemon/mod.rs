//! `vestige daemon` subcommand family.
//!
//! Lifecycle and observability for the V0.5 host-level daemon.

use clap::{Args, Subcommand};

pub mod kick;
pub mod log;
pub mod start;
pub mod status;
pub mod stop;

// === TYPES ===

#[derive(Args, Debug)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground (default).
    Start(start::StartArgs),
    /// Send SIGTERM to the running daemon (read from pidfile) and wait for exit.
    Stop(stop::StopArgs),
    /// Show daemon health, projects, and next-scheduled jobs.
    Status(status::StatusArgs),
    /// Run a job now (e.g. embed sweep across all projects).
    Kick(kick::KickArgs),
    /// Tail the daemon log file.
    Log(log::LogArgs),
}

// === PUBLIC API ===

pub fn run(args: DaemonArgs) -> anyhow::Result<()> {
    match args.command {
        DaemonCommand::Start(a) => start::run(a),
        DaemonCommand::Stop(a) => stop::run(a),
        DaemonCommand::Status(a) => status::run(a),
        DaemonCommand::Kick(a) => kick::run(a),
        DaemonCommand::Log(a) => log::run(a),
    }
}
