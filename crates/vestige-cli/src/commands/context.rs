use anyhow::Result;
use clap::Args;
use vestige_config::traces_config_for;
use vestige_engine::context::get_project_context;
use vestige_engine::Caller;

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
    let traces_cfg = traces_config_for(ctx.config.traces.as_ref());

    let outcome = get_project_context(
        &ctx.store,
        &ctx.project_id,
        &ctx.config.project_name,
        args.per_section,
        args.budget_tokens,
        Caller::Cli,
        &traces_cfg,
    )?;
    let pack = outcome.pack;

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
