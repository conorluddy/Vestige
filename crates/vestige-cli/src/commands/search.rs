use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{rank_hits, sanitize_fts_query, MemoryType, SearchFilter};

use crate::context;
use crate::output::{emit_json, print_scored, OutputFormat};

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Free-text query. Tokenised on whitespace; FTS5 special characters are
    /// stripped per token. Implicit AND across tokens.
    pub query: String,

    /// Filter by memory type.
    #[arg(long = "type", value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Cap the number of returned cards.
    #[arg(long, default_value_t = 8)]
    pub limit: u32,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: SearchArgs) -> Result<()> {
    let ctx = context::load()?;
    let r#type = args
        .r#type
        .as_deref()
        .map(MemoryType::from_str)
        .transpose()?;

    let cleaned = sanitize_fts_query(&args.query);
    let scored = if cleaned.is_empty() {
        Vec::new()
    } else {
        let hits = ctx.store.search_memories(
            &ctx.project_id,
            &cleaned,
            &SearchFilter {
                r#type,
                limit: Some(args.limit),
                ..Default::default()
            },
        )?;
        rank_hits(hits)
    };

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&scored),
        OutputFormat::Text => {
            if scored.is_empty() {
                println!("(no matches)");
            } else {
                for hit in &scored {
                    print_scored(hit);
                }
            }
            Ok(())
        }
    }
}
