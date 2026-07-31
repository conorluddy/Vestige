//! `vestige revise` — update a memory's content in place, keeping its handle.
//!
//! Thin dispatcher: parse `<id> <new_body>`, call `Store::revise_memory`,
//! format the outcome. No business logic here — see
//! `vestige_store::memory_ops::lifecycle::revise_memory` for the transaction.

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::MemoryId;

use crate::context;
use crate::output::{emit_json, OutputFormat};

/// Arguments for `vestige revise`.
#[derive(Debug, Args)]
pub struct ReviseArgs {
    /// Memory id to revise (`mem_<ULID>`).
    pub id: String,

    /// The new memory body. Replaces all four derived representations
    /// (`one_liner`/`summary`/`compressed`/`full`) in place.
    pub new_body: String,

    /// Emit JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ReviseArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let id = MemoryId::from_str(&args.id)?;
    let outcome = ctx.store.revise_memory(&id, &args.new_body)?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "id": id.to_string(),
            "prior_one_liner": outcome.prior_one_liner,
            "one_liner": outcome.new_one_liner,
        })),
        OutputFormat::Text => {
            println!("Revised {id}");
            println!("  was: {}", outcome.prior_one_liner);
            println!("  now: {}", outcome.new_one_liner);
            Ok(())
        }
    }
}
