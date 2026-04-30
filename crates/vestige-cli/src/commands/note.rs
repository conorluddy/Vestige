use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

#[derive(Debug, Args)]
pub struct NoteArgs {
    #[command(subcommand)]
    pub command: NoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum NoteCommand {
    /// Add a free-form note to the project memory.
    Add(CaptureAddArgs),
}

pub fn run(args: NoteArgs) -> Result<()> {
    match args.command {
        NoteCommand::Add(a) => capture::add(capture::NOTE, a),
    }
}
