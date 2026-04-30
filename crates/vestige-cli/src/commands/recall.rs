//! `vestige recall` — same retrieval engine as `search`, opinionated for
//! agent / user recall flows. PRD §12.6: "may apply more opinionated
//! ranking/filtering than raw search." For V0 the ranking is identical
//! (PRD §14.2) and the only difference is the default `--limit`, which
//! follows `recall.max_results` from `.vestige/config.toml`.
//!
//! V0.1 adds the same `--mode` / `--lexical` / `--semantic` / `--hybrid`
//! flags as `search`. The mode defaults to `lexical` (matching `search`),
//! respecting `[search] default_mode` in a future config extension (PR8).

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{resolve_default_mode, MemoryType, SearchMode};
use vestige_embed::{build_provider, EmbedError, EmbeddingProvider};
use vestige_engine::search::{search_hybrid, search_lexical, search_semantic, HybridOutcome};

use crate::context::{self, ProjectContext};
use crate::output::{emit_search_json, print_scored_opts, OutputFormat};

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
    let ctx = context::load()?;
    let type_filter = args
        .r#type
        .as_deref()
        .map(MemoryType::from_str)
        .transpose()?;
    let limit = args.limit.unwrap_or(ctx.config.recall.max_results);
    let mode = resolve_mode(&args, &ctx)?;
    let include_parts = args.score_parts || mode == SearchMode::Hybrid;
    let outcome = dispatch(&args.query, &ctx, type_filter, limit, mode)?;
    for w in &outcome.warnings {
        eprintln!("warning: {w}");
    }
    match OutputFormat::pick(args.json) {
        OutputFormat::Json => {
            emit_search_json(outcome.effective_mode, &outcome.scored, &outcome.warnings)
        }
        OutputFormat::Text => {
            print_scored_list(&outcome.scored, include_parts);
            Ok(())
        }
    }
}

// === HELPERS ===

fn dispatch(
    query: &str,
    ctx: &ProjectContext,
    type_filter: Option<MemoryType>,
    limit: u32,
    mode: SearchMode,
) -> Result<HybridOutcome> {
    match mode {
        SearchMode::Lexical => Ok(search_lexical(
            &ctx.store,
            &ctx.project_id,
            query,
            type_filter,
            limit,
        )?),
        SearchMode::Semantic => {
            let provider = build_embed_provider(ctx)?;
            Ok(search_semantic(
                &ctx.store,
                &ctx.project_id,
                query,
                type_filter,
                limit,
                &*provider,
            )?)
        }
        SearchMode::Hybrid => {
            let provider = build_embed_provider(ctx)?;
            Ok(search_hybrid(
                &ctx.store,
                &ctx.project_id,
                query,
                type_filter,
                limit,
                &*provider,
            )?)
        }
    }
}

/// Resolve the search mode from args.
///
/// Alias flags (`--lexical` / `--semantic` / `--hybrid`) take priority, then
/// `--mode`, then `[search] default_mode` from config, then `Lexical`.
fn resolve_mode(args: &RecallArgs, ctx: &ProjectContext) -> Result<SearchMode> {
    if args.lexical {
        return Ok(SearchMode::Lexical);
    }
    if args.semantic {
        return Ok(SearchMode::Semantic);
    }
    if args.hybrid {
        return Ok(SearchMode::Hybrid);
    }
    let config_default = ctx
        .config
        .search
        .as_ref()
        .and_then(|s| s.default_mode.as_deref());
    resolve_default_mode(args.mode.as_deref(), config_default).map_err(anyhow::Error::from)
}

fn build_embed_provider(ctx: &ProjectContext) -> Result<Box<dyn EmbeddingProvider>> {
    build_provider(&ctx.resolve_embeddings_config()).map_err(|e| {
        let hint = match &e {
            EmbedError::ProviderDisabled(name) => {
                format!("provider `{name}` is not compiled in; rebuild with `--features {name}`")
            }
            _ => e.to_string(),
        };
        anyhow::anyhow!("embedding provider error: {hint}")
    })
}

fn print_scored_list(scored: &[vestige_core::ScoredCard], include_parts: bool) {
    if scored.is_empty() {
        println!("(no matches)");
    } else {
        for hit in scored {
            print_scored_opts(hit, include_parts);
        }
    }
}
