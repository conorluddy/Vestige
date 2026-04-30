use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

#[derive(Debug, Args)]
pub struct DecisionArgs {
    #[command(subcommand)]
    pub command: DecisionCommand,
}

#[derive(Debug, Subcommand)]
pub enum DecisionCommand {
    /// Record a project decision.
    Add(CaptureAddArgs),
}

pub fn run(args: DecisionArgs) -> Result<()> {
    match args.command {
        DecisionCommand::Add(a) => capture::add(capture::DECISION, a),
    }
}
