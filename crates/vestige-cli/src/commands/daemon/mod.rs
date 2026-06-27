//! `vestige daemon` subcommand family.
//!
//! Lifecycle and observability for the V0.5 host-level daemon.

use clap::{Args, Subcommand};

pub mod doctor;
pub mod install;
pub mod ipc_client;
pub mod kick;
pub mod log;
pub mod pause;
pub mod restart;
pub mod resume;
pub mod start;
pub mod status;
pub mod stop;
pub mod uninstall;

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
    /// Bounce the daemon. Under launchd, uses `launchctl kickstart -k`.
    Restart(restart::RestartArgs),
    /// Show daemon health, projects, and next-scheduled jobs.
    Status(status::StatusArgs),
    /// Run a job now (e.g. embed sweep across all projects).
    Kick(kick::KickArgs),
    /// Pause scheduled ticks (`--for <dur>` or `--until <rfc3339>`).
    Pause(pause::PauseArgs),
    /// Resume scheduled ticks after a pause.
    Resume(resume::ResumeArgs),
    /// Tail the daemon log file.
    Log(log::LogArgs),
    /// Install the macOS LaunchAgent so the daemon starts at login.
    Install(install::InstallArgs),
    /// Uninstall the macOS LaunchAgent.
    Uninstall(uninstall::UninstallArgs),
    /// Run a comprehensive daemon health check.
    Doctor(doctor::DoctorArgs),
}

// === PUBLIC API ===

pub fn run(args: DaemonArgs) -> anyhow::Result<()> {
    match args.command {
        DaemonCommand::Start(a) => start::run(a),
        DaemonCommand::Stop(a) => stop::run(a),
        DaemonCommand::Restart(a) => restart::run(a),
        DaemonCommand::Status(a) => status::run(a),
        DaemonCommand::Kick(a) => kick::run(a),
        DaemonCommand::Pause(a) => pause::run(a),
        DaemonCommand::Resume(a) => resume::run(a),
        DaemonCommand::Log(a) => log::run(a),
        DaemonCommand::Install(a) => install::run(a),
        DaemonCommand::Uninstall(a) => uninstall::run(a),
        DaemonCommand::Doctor(a) => doctor::run(a),
    }
}
