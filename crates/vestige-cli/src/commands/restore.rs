use std::str::FromStr;

use anyhow::{anyhow, Result};
use clap::Args;
use vestige_core::{CoreError, MemoryId};

use crate::context;
use crate::output::{emit_json, OutputFormat};

#[derive(Debug, Args)]
pub struct RestoreArgs {
    pub id: String,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: RestoreArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let id = MemoryId::from_str(&args.id)?;
    let flipped = ctx.store.restore_memory(&id)?;
    if !flipped {
        return Err(anyhow!(CoreError::MemoryNotFound(format!(
            "{id} (or not in deleted state)"
        ))));
    }

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "id": id.to_string(),
            "status": "active",
        })),
        OutputFormat::Text => {
            println!("Restored {id}");
            Ok(())
        }
    }
}
