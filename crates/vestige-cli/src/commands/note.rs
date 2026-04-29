use anyhow::Result;
use clap::{Args, Subcommand};
use vestige_core::MemoryType;

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

#[derive(Debug, Args)]
pub struct NoteArgs {
    #[command(subcommand)]
    pub command: NoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum NoteCommand {
    /// Add a free-form note to the project memory.
    Add(NoteAddArgs),
}

#[derive(Debug, Args)]
pub struct NoteAddArgs {
    pub body: String,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub source_content: Option<String>,
    #[arg(long, default_value_t = 0.5)]
    pub importance: f64,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: NoteArgs) -> Result<()> {
    match args.command {
        NoteCommand::Add(a) => add(a),
    }
}

fn add(args: NoteAddArgs) -> Result<()> {
    let mut ctx = context::load()?;
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::Note,
            body: &args.body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
