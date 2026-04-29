use std::str::FromStr;

use anyhow::{anyhow, Result};
use clap::Args;
use vestige_core::{CoreError, MemoryId};

use crate::context;
use crate::output::{emit_json, OutputFormat};

#[derive(Debug, Args)]
pub struct ForgetArgs {
    pub id: String,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ForgetArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let id = MemoryId::from_str(&args.id)?;
    let flipped = ctx.store.forget_memory(&id)?;
    if !flipped {
        return Err(anyhow!(CoreError::MemoryNotFound(format!(
            "{id} (or already deleted)"
        ))));
    }

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "id": id.to_string(),
            "status": "deleted",
        })),
        OutputFormat::Text => {
            println!("Forgot {id} (soft delete; restorable with `vestige restore`)");
            Ok(())
        }
    }
}
