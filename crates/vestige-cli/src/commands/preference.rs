use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

#[derive(Debug, Args)]
pub struct PreferenceArgs {
    #[command(subcommand)]
    pub command: PreferenceCommand,
}

#[derive(Debug, Subcommand)]
pub enum PreferenceCommand {
    /// Record a project-scoped preference.
    Add(CaptureAddArgs),
}

pub fn run(args: PreferenceArgs) -> Result<()> {
    match args.command {
        PreferenceCommand::Add(a) => capture::add(capture::PREFERENCE, a),
    }
}
