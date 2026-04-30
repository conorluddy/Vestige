//! `vestige recall` — same retrieval engine as `search`, opinionated for
//! agent / user recall flows. PRD §12.6: "may apply more opinionated
//! ranking/filtering than raw search." For V0 the ranking is identical
//! (PRD §14.2) and the only difference is the default `--limit`, which
//! follows `recall.max_results` from `.vestige/config.toml`.

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{rank_hits, sanitize_fts_query, MemoryType, SearchFilter};

use crate::context;
use crate::output::{emit_json, print_scored, OutputFormat};

#[derive(Debug, Args)]
pub struct RecallArgs {
    pub query: String,

    #[arg(long = "type", value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Override the configured `recall.max_results`.
    #[arg(long)]
    pub limit: Option<u32>,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: RecallArgs) -> Result<()> {
    let ctx = context::load()?;
    let r#type = args
        .r#type
        .as_deref()
        .map(MemoryType::from_str)
        .transpose()?;
    let limit = args.limit.unwrap_or(ctx.config.recall.max_results);

    let cleaned = sanitize_fts_query(&args.query);
    let scored = if cleaned.is_empty() {
        Vec::new()
    } else {
        let hits = ctx.store.search_memories(
            &ctx.project_id,
            &cleaned,
            &SearchFilter {
                r#type,
                limit: Some(limit),
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
