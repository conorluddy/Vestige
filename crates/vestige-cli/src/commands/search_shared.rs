//! Shared dispatcher for `vestige search` and `vestige recall`.
//!
//! The two commands differ only in their `--limit` defaulting strategy
//! (constant `8` for `search`, `[recall] max_results` from config for `recall`).
//! Everything else — mode resolution, provider construction, dispatch into
//! `vestige_engine::search::*`, warning forwarding, JSON envelope, and text
//! printing — is identical.
//!
//! `run_search` is the single entry point; `search.rs` and `recall.rs` reduce
//! to clap `Args` structs plus a one-liner `run` that resolves their `limit`
//! default and delegates here.

use std::str::FromStr;

use anyhow::Result;
use vestige_core::{resolve_default_mode, MemoryType, SearchMode};
use vestige_embed::{build_provider, EmbedError, EmbeddingProvider};
use vestige_engine::search::{search_hybrid, search_lexical, search_semantic, HybridOutcome};

use crate::context::ProjectContext;
use crate::output::{emit_search_json, print_scored_opts, OutputFormat};

// === TYPES ===

/// Mode-selection flags shared by both `search` and `recall`. The convenience
/// alias flags (`--lexical` / `--semantic` / `--hybrid`) are clap-checked
/// mutually exclusive at the call site; this struct just relays their values.
pub struct SearchModeFlags<'a> {
    pub mode: Option<&'a str>,
    pub lexical: bool,
    pub semantic: bool,
    pub hybrid: bool,
    pub score_parts: bool,
}

// === PUBLIC API ===

/// Execute a search with already-resolved `limit` and `mode_flags`.
///
/// Loads the project context, resolves the search mode (alias flags →
/// `--mode` → config default → `Lexical`), dispatches into the engine,
/// forwards any warnings to stderr, and prints the result envelope in
/// the requested format.
pub fn run_search(
    query: &str,
    type_filter: Option<MemoryType>,
    limit: u32,
    mode_flags: SearchModeFlags<'_>,
    json: bool,
) -> Result<()> {
    let ctx = crate::context::load()?;
    let mode = resolve_mode(&mode_flags, &ctx)?;
    let include_parts = mode_flags.score_parts || mode == SearchMode::Hybrid;
    let outcome = dispatch(query, &ctx, type_filter, limit, mode)?;
    for w in &outcome.warnings {
        eprintln!("warning: {w}");
    }
    match OutputFormat::pick(json) {
        OutputFormat::Json => {
            emit_search_json(outcome.effective_mode, &outcome.scored, &outcome.warnings)
        }
        OutputFormat::Text => {
            print_scored_list(&outcome.scored, &outcome.warnings, include_parts);
            Ok(())
        }
    }
}

/// Parse a `--type <T>` argument into an optional [`MemoryType`].
pub fn parse_type_filter(raw: Option<&str>) -> Result<Option<MemoryType>> {
    raw.map(MemoryType::from_str)
        .transpose()
        .map_err(Into::into)
}

// === PRIVATE HELPERS ===

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

/// Resolve the search mode from flags, config default, and the engine fallback.
///
/// Alias flags (`--lexical` / `--semantic` / `--hybrid`) take priority, then
/// `--mode`, then `[search] default_mode` from config, then `Lexical`.
fn resolve_mode(flags: &SearchModeFlags<'_>, ctx: &ProjectContext) -> Result<SearchMode> {
    if flags.lexical {
        return Ok(SearchMode::Lexical);
    }
    if flags.semantic {
        return Ok(SearchMode::Semantic);
    }
    if flags.hybrid {
        return Ok(SearchMode::Hybrid);
    }
    let config_default = ctx
        .config
        .search
        .as_ref()
        .and_then(|s| s.default_mode.as_deref());
    resolve_default_mode(flags.mode, config_default).map_err(anyhow::Error::from)
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

fn print_scored_list(
    scored: &[vestige_core::ScoredCard],
    warnings: &[String],
    include_parts: bool,
) {
    if scored.is_empty() {
        // Cold-start polish: when the engine emitted a warning pointing at
        // `vestige embed --all`, inline the actionable hint with "(no matches)"
        // so a reader who skims past the stderr warning still sees it.
        let cold_start = warnings.iter().any(|w| w.contains("vestige embed --all"));
        if cold_start {
            println!("(no matches — try `vestige embed --all` to enable semantic recall)");
        } else {
            println!("(no matches)");
        }
    } else {
        for hit in scored {
            print_scored_opts(hit, include_parts);
        }
    }
}
