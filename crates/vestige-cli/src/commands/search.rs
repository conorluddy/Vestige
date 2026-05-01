//! `vestige search` — lexical, semantic, or hybrid memory retrieval.
//!
//! Default mode is `lexical` (FTS5, always available) unless the caller
//! supplies `--mode`, `--semantic`, or `--hybrid`, or config sets
//! `[search] default_mode`. The `--limit` default is fixed in clap (8).
//! For a config-budgeted limit per project, use `vestige recall` (PRD §12.6).
//!
//! All real work lives in [`crate::commands::search_shared`]; this file is
//! a thin clap shell.

use anyhow::Result;
use clap::Args;

use crate::commands::search_shared::{parse_type_filter, run_search, SearchModeFlags};

// === ARGS ===

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
    /// Output JSON instead of human-readable text.
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

pub fn run(args: SearchArgs) -> Result<()> {
    let type_filter = parse_type_filter(args.r#type.as_deref())?;
    let mode_flags = SearchModeFlags {
        mode: args.mode.as_deref(),
        lexical: args.lexical,
        semantic: args.semantic,
        hybrid: args.hybrid,
        score_parts: args.score_parts,
    };
    run_search(&args.query, type_filter, args.limit, mode_flags, args.json)
}
