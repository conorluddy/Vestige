use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

#[derive(Debug, Args)]
pub struct QuestionArgs {
    #[command(subcommand)]
    pub command: QuestionCommand,
}

#[derive(Debug, Subcommand)]
pub enum QuestionCommand {
    /// Record an open question to revisit later.
    Add(CaptureAddArgs),
}

pub fn run(args: QuestionArgs) -> Result<()> {
    match args.command {
        QuestionCommand::Add(a) => capture::add(capture::QUESTION, a),
    }
}
