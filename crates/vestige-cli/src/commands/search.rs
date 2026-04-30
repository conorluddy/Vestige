//! `vestige search` — lexical, semantic, or hybrid memory retrieval.
//!
//! Default mode is `lexical` (FTS5, always available) unless the caller
//! supplies `--mode`, `--semantic`, or `--hybrid`, or config sets
//! `[search] default_mode`.

use std::collections::HashSet;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::Args;
use vestige_core::{
    merge_hits, normalise_cosine, normalise_fts, rank_hits, sanitize_fts_query, HybridOpts,
    MemoryType, SearchFilter, SearchHit, SearchMode, SemanticHit,
};
use vestige_embed::build_provider;
use vestige_store::VectorFilter;

use crate::context;
use crate::output::{emit_search_json, print_scored_opts, OutputFormat};

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
    let ctx = context::load()?;
    let type_filter = args
        .r#type
        .as_deref()
        .map(MemoryType::from_str)
        .transpose()?;

    let mode = resolve_mode(&args, &ctx)?;
    let include_parts = args.score_parts || mode == SearchMode::Hybrid;

    match mode {
        SearchMode::Lexical => run_lexical(args, ctx, type_filter, include_parts),
        SearchMode::Semantic => run_semantic(args, ctx, type_filter, include_parts),
        SearchMode::Hybrid => run_hybrid(args, ctx, type_filter),
    }
}

// === MODE DISPATCH ===

fn run_lexical(
    args: SearchArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
    include_parts: bool,
) -> Result<()> {
    let cleaned = sanitize_fts_query(&args.query);
    let scored = if cleaned.is_empty() {
        Vec::new()
    } else {
        let hits = ctx.store.search_memories(
            &ctx.project_id,
            &cleaned,
            &SearchFilter {
                r#type: type_filter,
                limit: Some(args.limit),
                ..Default::default()
            },
        )?;
        rank_hits(hits)
    };

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_search_json(SearchMode::Lexical, &scored, &[]),
        OutputFormat::Text => {
            print_scored_list(&scored, include_parts);
            Ok(())
        }
    }
}

fn run_semantic(
    args: SearchArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
    include_parts: bool,
) -> Result<()> {
    let provider = build_embed_provider(&ctx)?;

    // Check if embeddings exist before running the query.
    let status = ctx.store.embedding_status(&ctx.project_id)?;
    if status.embedded_representations == 0 {
        eprintln!("No embeddings found for this project. Run `vestige embed --all` first.");
        match OutputFormat::pick(args.json) {
            OutputFormat::Json => emit_search_json(
                SearchMode::Semantic,
                &[],
                &["no embeddings; run `vestige embed --all` first".to_string()],
            ),
            OutputFormat::Text => Ok(()),
        }
    } else {
        let query_vec = provider
            .embed(&args.query)
            .context("embedding the search query")?;
        let filter = VectorFilter {
            provider: provider.provider_name().to_string(),
            model: provider.model_name().to_string(),
            dimensions: provider.dimensions(),
            memory_type: type_filter,
        };
        let raw_hits =
            ctx.store
                .nearest_neighbours(&ctx.project_id, &query_vec, args.limit, &filter)?;

        // Map VectorHit → ScoredCard by hydrating each memory from the store.
        let mut scored = Vec::with_capacity(raw_hits.len());
        for hit in &raw_hits {
            if let Some(fetched) = ctx.store.get_memory(&hit.memory_id)? {
                let similarity = hit.similarity.clamp(0.0, 1.0);
                let card = vestige_core::project_card(&fetched);
                scored.push(vestige_core::ScoredCard {
                    card,
                    score: similarity,
                    score_parts: None,
                });
            }
        }

        match OutputFormat::pick(args.json) {
            OutputFormat::Json => emit_search_json(SearchMode::Semantic, &scored, &[]),
            OutputFormat::Text => {
                print_scored_list(&scored, include_parts);
                Ok(())
            }
        }
    }
}

fn run_hybrid(
    args: SearchArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
) -> Result<()> {
    // Check if any embeddings exist; fall back to lexical if not.
    let status = ctx.store.embedding_status(&ctx.project_id)?;
    if status.embedded_representations == 0 {
        let warning = "no embeddings; hybrid falling back to lexical (run `vestige embed --all` to enable semantic recall)".to_string();
        eprintln!("warning: {warning}");

        let cleaned = sanitize_fts_query(&args.query);
        let scored = if cleaned.is_empty() {
            Vec::new()
        } else {
            let hits = ctx.store.search_memories(
                &ctx.project_id,
                &cleaned,
                &SearchFilter {
                    r#type: type_filter,
                    limit: Some(args.limit),
                    ..Default::default()
                },
            )?;
            rank_hits(hits)
        };

        return match OutputFormat::pick(args.json) {
            OutputFormat::Json => emit_search_json(SearchMode::Hybrid, &scored, &[warning]),
            OutputFormat::Text => {
                print_scored_list(&scored, false);
                Ok(())
            }
        };
    }

    // --- Lexical leg ---
    let cleaned = sanitize_fts_query(&args.query);
    let lexical_hits: Vec<SearchHit> = if cleaned.is_empty() {
        Vec::new()
    } else {
        ctx.store.search_memories(
            &ctx.project_id,
            &cleaned,
            &SearchFilter {
                r#type: type_filter,
                // Over-fetch for the merge; core applies limit after merge.
                limit: Some(args.limit.saturating_mul(4).max(32)),
                ..Default::default()
            },
        )?
    };

    // --- Semantic leg ---
    let provider = build_embed_provider(&ctx)?;
    let query_vec = provider
        .embed(&args.query)
        .context("embedding query for hybrid search")?;
    let vector_filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let vector_raw = ctx.store.nearest_neighbours(
        &ctx.project_id,
        &query_vec,
        args.limit.saturating_mul(4).max(32),
        &vector_filter,
    )?;

    // Map VectorHit → SemanticHit (core-side type).
    let semantic_hits: Vec<SemanticHit> = vector_raw
        .iter()
        .map(|h| SemanticHit {
            memory_id: h.memory_id.clone(),
            representation_type: h.representation_type.clone(),
            similarity: h.similarity,
        })
        .collect();

    // --- Build the unified candidate set ---
    // Start with all lexical hits.
    let mut seen_ids: HashSet<String> = lexical_hits
        .iter()
        .map(|h| h.fetched.memory.id.as_str().to_string())
        .collect();

    let mut candidates: Vec<SearchHit> = lexical_hits.clone();

    // For memories that only appeared in the semantic leg, hydrate them from
    // the store and synthesise a SearchHit with bm25 = 0.0.
    for sem_hit in &semantic_hits {
        let id_str = sem_hit.memory_id.as_str().to_string();
        if seen_ids.contains(&id_str) {
            continue;
        }
        seen_ids.insert(id_str);
        if let Some(fetched) = ctx.store.get_memory(&sem_hit.memory_id)? {
            candidates.push(SearchHit { fetched, bm25: 0.0 });
        }
    }

    // Normalise both scoring dimensions.
    let fts_scores = normalise_fts(&lexical_hits);
    let vector_scores = normalise_cosine(&semantic_hits);

    let opts = HybridOpts {
        limit: args.limit,
        ..HybridOpts::default()
    };
    let scored = merge_hits(candidates, &fts_scores, &vector_scores, &opts);

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_search_json(SearchMode::Hybrid, &scored, &[]),
        OutputFormat::Text => {
            print_scored_list(&scored, true);
            Ok(())
        }
    }
}

// === HELPERS ===

/// Resolve the search mode from args, with aliases taking priority over
/// `--mode`, then falling back to `[search] default_mode` from config,
/// then `lexical`.
fn resolve_mode(args: &SearchArgs, ctx: &context::ProjectContext) -> Result<SearchMode> {
    if args.lexical {
        return Ok(SearchMode::Lexical);
    }
    if args.semantic {
        return Ok(SearchMode::Semantic);
    }
    if args.hybrid {
        return Ok(SearchMode::Hybrid);
    }
    if let Some(ref mode_str) = args.mode {
        return SearchMode::from_str(mode_str).map_err(anyhow::Error::from);
    }
    if let Some(mode) = ctx
        .config
        .search
        .as_ref()
        .and_then(|s| s.default_mode.as_deref())
        .map(SearchMode::from_str)
        .transpose()?
    {
        return Ok(mode);
    }
    Ok(SearchMode::Lexical)
}

/// Build an embedding provider from the project config, or fall back to `fake`
/// when the config has no `[embeddings]` section. Errors are surfaced as
/// actionable `anyhow` messages.
fn build_embed_provider(
    ctx: &context::ProjectContext,
) -> Result<Box<dyn vestige_embed::EmbeddingProvider>> {
    let cfg = ctx.resolve_embeddings_config();
    build_provider(&cfg).map_err(|e| {
        let hint = match &e {
            vestige_embed::EmbedError::ProviderDisabled(name) => {
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
