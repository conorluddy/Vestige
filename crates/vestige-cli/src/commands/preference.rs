use anyhow::Result;
use clap::{Args, Subcommand};
use vestige_core::MemoryType;

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

#[derive(Debug, Args)]
pub struct PreferenceArgs {
    #[command(subcommand)]
    pub command: PreferenceCommand,
}

#[derive(Debug, Subcommand)]
pub enum PreferenceCommand {
    /// Record a project-scoped preference.
    Add(PreferenceAddArgs),
}

#[derive(Debug, Args)]
pub struct PreferenceAddArgs {
    pub body: String,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub source_content: Option<String>,
    #[arg(long, default_value_t = 0.6)]
    pub importance: f64,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: PreferenceArgs) -> Result<()> {
    match args.command {
        PreferenceCommand::Add(a) => add(a),
    }
}

fn add(args: PreferenceAddArgs) -> Result<()> {
    let mut ctx = context::load()?;
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::Preference,
            body: &args.body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
