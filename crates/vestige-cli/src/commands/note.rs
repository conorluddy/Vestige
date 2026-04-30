//! `vestige note` — capture a free-form note.
//!
//! Thin dispatcher: `vestige note add <body>` maps to [`capture::add`] with
//! [`capture::NOTE`] (type: `note`, default importance 0.5).

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::commands::capture::{self, CaptureAddArgs};

/// Arguments for `vestige note`.
#[derive(Debug, Args)]
pub struct NoteArgs {
    #[command(subcommand)]
    pub command: NoteCommand,
}

/// Subcommands for `vestige note`.
#[derive(Debug, Subcommand)]
pub enum NoteCommand {
    /// Add a free-form note to the project memory.
    Add(CaptureAddArgs),
}

/// Dispatch to the `note` subcommand handler.
pub fn run(args: NoteArgs) -> Result<()> {
    match args.command {
        NoteCommand::Add(a) => capture::add(capture::NOTE, a),
    }
}
