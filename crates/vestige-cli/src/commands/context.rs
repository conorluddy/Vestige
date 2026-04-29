use anyhow::Result;
use clap::Args;
use vestige_core::{build_pack, ContextOptions, ContextSources, ListFilter, MemoryType};

use crate::context as cli_ctx;
use crate::output::{emit_json, OutputFormat};

#[derive(Debug, Args)]
pub struct ContextArgs {
    /// Approximate token budget for the assembled pack. Sections are skipped
    /// once the budget is exhausted.
    #[arg(long, default_value_t = 1200)]
    pub budget_tokens: usize,

    /// Cap each list section (decisions / open questions / recent).
    #[arg(long, default_value_t = 8)]
    pub per_section: u32,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ContextArgs) -> Result<()> {
    let ctx = cli_ctx::load()?;

    let summary = ctx
        .store
        .list_memories(
            &ctx.project_id,
            &ListFilter {
                include_deleted: false,
                r#type: Some(MemoryType::ProjectSummary),
                limit: Some(1),
            },
        )?
        .into_iter()
        .next();

    let decisions = ctx.store.list_memories(
        &ctx.project_id,
        &ListFilter {
            include_deleted: false,
            r#type: Some(MemoryType::Decision),
            limit: Some(args.per_section),
        },
    )?;
    let open_questions = ctx.store.list_memories(
        &ctx.project_id,
        &ListFilter {
            include_deleted: false,
            r#type: Some(MemoryType::OpenQuestion),
            limit: Some(args.per_section),
        },
    )?;
    let recent = ctx.store.list_memories(
        &ctx.project_id,
        &ListFilter {
            include_deleted: false,
            r#type: None,
            limit: Some(args.per_section),
        },
    )?;

    let pack = build_pack(
        ContextSources {
            project_name: ctx.config.project_name.clone(),
            summary,
            decisions,
            open_questions,
            recent,
        },
        ContextOptions {
            budget_tokens: args.budget_tokens,
        },
    );

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&pack),
        OutputFormat::Text => {
            print!("{}", pack.text);
            if pack.truncated {
                eprintln!(
                    "warning: context pack truncated to fit budget of {} tokens (~{} chars)",
                    args.budget_tokens,
                    pack.text.len()
                );
            }
            Ok(())
        }
    }
}
