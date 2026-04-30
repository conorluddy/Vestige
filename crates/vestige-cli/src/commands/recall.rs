//! `vestige recall` — same retrieval engine as `search`, opinionated for
//! agent / user recall flows. PRD §12.6: "may apply more opinionated
//! ranking/filtering than raw search." For V0 the ranking is identical
//! (PRD §14.2) and the only difference is the default `--limit`, which
//! follows `recall.max_results` from `.vestige/config.toml`.
//!
//! V0.1 adds the same `--mode` / `--lexical` / `--semantic` / `--hybrid`
//! flags as `search`. The mode defaults to `lexical` (matching `search`),
//! respecting `[search] default_mode` in a future config extension (PR8).

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
    let mode = resolve_mode(&args)?;
    let include_parts = args.score_parts || mode == SearchMode::Hybrid;

    match mode {
        SearchMode::Lexical => run_lexical(args, ctx, type_filter, limit, include_parts),
        SearchMode::Semantic => run_semantic(args, ctx, type_filter, limit, include_parts),
        SearchMode::Hybrid => run_hybrid(args, ctx, type_filter, limit),
    }
}

// === MODE DISPATCH ===

fn run_lexical(
    args: RecallArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
    limit: u32,
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
                limit: Some(limit),
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
    args: RecallArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
    limit: u32,
    include_parts: bool,
) -> Result<()> {
    let provider = build_embed_provider(&ctx)?;

    let status = ctx.store.embedding_status(&ctx.project_id)?;
    if status.embedded_representations == 0 {
        eprintln!("No embeddings found for this project. Run `vestige embed --all` first.");
        return match OutputFormat::pick(args.json) {
            OutputFormat::Json => emit_search_json(
                SearchMode::Semantic,
                &[],
                &["no embeddings; run `vestige embed --all` first".to_string()],
            ),
            OutputFormat::Text => Ok(()),
        };
    }

    let query_vec = provider
        .embed(&args.query)
        .context("embedding the recall query")?;
    let filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let raw_hits = ctx
        .store
        .nearest_neighbours(&ctx.project_id, &query_vec, limit, &filter)?;

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

fn run_hybrid(
    args: RecallArgs,
    ctx: context::ProjectContext,
    type_filter: Option<MemoryType>,
    limit: u32,
) -> Result<()> {
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
                    limit: Some(limit),
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
                limit: Some(limit.saturating_mul(4).max(32)),
                ..Default::default()
            },
        )?
    };

    // --- Semantic leg ---
    let provider = build_embed_provider(&ctx)?;
    let query_vec = provider
        .embed(&args.query)
        .context("embedding query for hybrid recall")?;
    let vector_filter = VectorFilter {
        provider: provider.provider_name().to_string(),
        model: provider.model_name().to_string(),
        dimensions: provider.dimensions(),
        memory_type: type_filter,
    };
    let vector_raw = ctx.store.nearest_neighbours(
        &ctx.project_id,
        &query_vec,
        limit.saturating_mul(4).max(32),
        &vector_filter,
    )?;

    let semantic_hits: Vec<SemanticHit> = vector_raw
        .iter()
        .map(|h| SemanticHit {
            memory_id: h.memory_id.clone(),
            representation_type: h.representation_type.clone(),
            similarity: h.similarity,
        })
        .collect();

    // Union candidate set.
    let mut seen_ids: HashSet<String> = lexical_hits
        .iter()
        .map(|h| h.fetched.memory.id.as_str().to_string())
        .collect();
    let mut candidates: Vec<SearchHit> = lexical_hits.clone();

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

    let fts_scores = normalise_fts(&lexical_hits);
    let vector_scores = normalise_cosine(&semantic_hits);

    let opts = HybridOpts {
        limit,
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

fn resolve_mode(args: &RecallArgs) -> Result<SearchMode> {
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
    Ok(SearchMode::Lexical)
}

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
