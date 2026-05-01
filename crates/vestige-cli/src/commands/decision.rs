//! `vestige decision` — record an architectural or project decision.
//!
//! Thin dispatcher: `vestige decision add <body> [--rationale <text>]` maps to
//! [`capture::add`] with [`capture::DECISION`] (type: `decision`, default
//! importance 0.7, rationale appended as `"\n\nRationale: <text>"`).

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

/// Arguments for `vestige decision`.
#[derive(Debug, Args)]
pub struct DecisionArgs {
    #[command(subcommand)]
    pub command: DecisionCommand,
}

/// Subcommands for `vestige decision`.
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
