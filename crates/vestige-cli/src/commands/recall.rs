//! `vestige recall` — agent-friendly memory retrieval. Same engine as
//! `search` (`vestige_engine::search_*`); the only difference is `--limit`
//! defaults to `[recall] max_results` from `.vestige/config.toml` (PRD §12.6),
//! whereas `search`'s default is fixed in clap. Keep both: agents pin a
//! per-project recall budget via config; humans pass `--limit` explicitly.
//!
//! All real work lives in [`crate::commands::search_shared`]; this file is
//! a thin clap shell that resolves the config-default limit then delegates.

use anyhow::Result;
use clap::Args;

use crate::commands::search_shared::{parse_type_filter, run_search, SearchModeFlags};
use crate::context;

// === ARGS ===

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
    /// Search mode: lexical | semantic | hybrid.
    /// Mutually exclusive with --lexical / --semantic / --hybrid.
    #[arg(long = "mode", value_name = "MODE", conflicts_with_all = ["lexical", "semantic", "hybrid"])]
    pub mode: Option<String>,
    /// Convenience alias for --mode lexical.
    #[arg(long, conflicts_with_all = ["mode", "semantic", "hybrid"])]
    pub lexical: bool,
    /// Convenience alias for --mode semantic.
    #[arg(long, conflicts_with_all = ["mode", "lexical", "hybrid"])]
    pub semantic: bool,
    /// Convenience alias for --mode hybrid.
    #[arg(long, conflicts_with_all = ["mode", "lexical", "semantic"])]
    pub hybrid: bool,
    /// Include score component breakdown in JSON output.
    /// Automatically enabled for hybrid mode.
    #[arg(long)]
    pub score_parts: bool,
}

// === ENTRY POINT ===

pub fn run(args: RecallArgs) -> Result<()> {
    // Resolve the config-default limit before dispatching. We deliberately
    // load the context twice (here for the limit, again inside run_search)
    // so the shared dispatcher stays limit-agnostic; the cost is one extra
    // SQLite open against an already-WAL DB (microseconds).
    let limit = match args.limit {
        Some(n) => n,
        None => context::load()?.config.recall.max_results,
    };
    let type_filter = parse_type_filter(args.r#type.as_deref())?;
    let mode_flags = SearchModeFlags {
        mode: args.mode.as_deref(),
        lexical: args.lexical,
        semantic: args.semantic,
        hybrid: args.hybrid,
        score_parts: args.score_parts,
    };
    run_search(&args.query, type_filter, limit, mode_flags, args.json)
}
