use anyhow::Result;
use clap::Args;
use vestige_core::{project_card, ListFilter, MemoryType};

use crate::context;
use crate::output::{emit_json, print_card, OutputFormat};

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by memory type (decision, note, observation, preference,
    /// project_summary, open_question).
    #[arg(long = "type", value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Include soft-deleted memories.
    #[arg(long)]
    pub include_deleted: bool,

    /// Cap the number of returned memories.
    #[arg(long, default_value_t = 50)]
    pub limit: u32,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ListArgs) -> Result<()> {
    let ctx = context::load()?;
    let r#type = args
        .r#type
        .as_deref()
        .map(<MemoryType as std::str::FromStr>::from_str)
        .transpose()?;

    let filter = ListFilter {
        include_deleted: args.include_deleted,
        r#type,
        limit: Some(args.limit),
    };

    let fetched = ctx.store.list_memories(&ctx.project_id, &filter)?;
    let cards: Vec<_> = fetched.iter().map(project_card).collect();

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&cards),
        OutputFormat::Text => {
            if cards.is_empty() {
                println!("(no memories)");
            } else {
                for card in &cards {
                    print_card(card);
                }
            }
            Ok(())
        }
    }
}
