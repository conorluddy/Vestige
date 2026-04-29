use std::str::FromStr;

use anyhow::{anyhow, Result};
use clap::Args;
use vestige_core::{project_detail, CoreError, MemoryId, RepresentationDepth};

use crate::context;
use crate::output::{emit_json, print_detail, OutputFormat};

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Memory id (e.g. mem_01HXXXXXXXXXXXXXXXXXX).
    pub id: String,

    /// Representation depth: one_liner, summary, compressed, full.
    #[arg(long, default_value = "summary")]
    pub depth: String,

    /// Include attached source rows.
    #[arg(long)]
    pub sources: bool,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ShowArgs) -> Result<()> {
    let ctx = context::load()?;
    let id = MemoryId::from_str(&args.id)?;
    let depth = RepresentationDepth::from_str(&args.depth)?;

    let fetched = ctx
        .store
        .get_memory(&id)?
        .ok_or_else(|| anyhow!(CoreError::MemoryNotFound(id.to_string())))?;
    let detail = project_detail(&fetched);

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&detail),
        OutputFormat::Text => {
            print_detail(&detail, depth, args.sources);
            Ok(())
        }
    }
}
