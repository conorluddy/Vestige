use anyhow::Result;
use clap::{Args, Subcommand};
use vestige_core::MemoryType;

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

#[derive(Debug, Args)]
pub struct DecisionArgs {
    #[command(subcommand)]
    pub command: DecisionCommand,
}

#[derive(Debug, Subcommand)]
pub enum DecisionCommand {
    /// Record a project decision.
    Add(DecisionAddArgs),
}

#[derive(Debug, Args)]
pub struct DecisionAddArgs {
    pub decision: String,
    /// Optional rationale appended to the decision body.
    #[arg(long)]
    pub rationale: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub source_content: Option<String>,
    /// Decisions default to higher importance than free-form notes.
    #[arg(long, default_value_t = 0.7)]
    pub importance: f64,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: DecisionArgs) -> Result<()> {
    match args.command {
        DecisionCommand::Add(a) => add(a),
    }
}

fn add(args: DecisionAddArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let body = match args.rationale.as_deref() {
        Some(r) => format!("{}\n\nRationale: {}", args.decision, r),
        None => args.decision.clone(),
    };
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::Decision,
            body: &body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
