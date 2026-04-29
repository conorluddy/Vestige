use anyhow::Result;
use clap::{Args, Subcommand};
use vestige_core::MemoryType;

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

#[derive(Debug, Args)]
pub struct QuestionArgs {
    #[command(subcommand)]
    pub command: QuestionCommand,
}

#[derive(Debug, Subcommand)]
pub enum QuestionCommand {
    /// Record an open question to revisit later.
    Add(QuestionAddArgs),
}

#[derive(Debug, Args)]
pub struct QuestionAddArgs {
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

pub fn run(args: QuestionArgs) -> Result<()> {
    match args.command {
        QuestionCommand::Add(a) => add(a),
    }
}

fn add(args: QuestionAddArgs) -> Result<()> {
    let mut ctx = context::load()?;
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::OpenQuestion,
            body: &args.body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
