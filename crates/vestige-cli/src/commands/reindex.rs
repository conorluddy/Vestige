//! `vestige reindex` — rebuild FTS and/or embedding indexes.
//!
//! Embeddings are a disposable acceleration layer; they can always be rebuilt
//! from the durable `memories` + `memory_representations` journal.
//!
//! `--fts`:        FTS5 `rebuild` command (SQLite shadow table reconstruction).
//! `--embeddings`: generate replacement vectors, then atomically swap the index.
//! `--all`:        both, in order.

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

use vestige_engine::embed;

use crate::commands::embed::{build_summary, EmbedSummary, EmbedTarget};
use crate::context;
use crate::output::{emit_json, OutputFormat};

// === TYPES ===

#[derive(Debug, Serialize)]
pub struct ReindexSummary {
    pub fts_rebuilt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<EmbedSummary>,
}

// === CLI ARGS ===

#[derive(Debug, Args)]
pub struct ReindexArgs {
    /// Rebuild the FTS5 full-text search index.
    #[arg(long)]
    pub fts: bool,

    /// Re-embed from scratch, preserving the previous index if rebuilding fails.
    #[arg(long)]
    pub embeddings: bool,

    /// Rebuild both the FTS index and all embeddings.
    #[arg(long, conflicts_with_all = ["fts", "embeddings"])]
    pub all: bool,

    /// Override the embedding provider when rebuilding embeddings.
    #[arg(long)]
    pub provider: Option<String>,

    /// Override the model name.
    #[arg(long)]
    pub model: Option<String>,

    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: ReindexArgs) -> Result<()> {
    if !args.fts && !args.embeddings && !args.all {
        anyhow::bail!("one of --fts, --embeddings, or --all is required");
    }

    let do_fts = args.all || args.fts;
    let do_embeddings = args.all || args.embeddings;

    let mut ctx = context::load()?;
    let provider = if do_embeddings {
        Some(ctx.embedding_provider(args.provider.as_deref(), args.model.as_deref())?)
    } else {
        None
    };

    let mut fts_rebuilt = false;
    let mut embed_summary: Option<EmbedSummary> = None;

    if do_fts {
        ctx.store.rebuild_fts().context("rebuilding FTS5 index")?;
        fts_rebuilt = true;
        tracing::info!("FTS5 index rebuilt");
    }

    if do_embeddings {
        let depths = vec![
            vestige_core::RepresentationDepth::Summary,
            vestige_core::RepresentationDepth::Compressed,
        ];

        let provider = provider
            .as_deref()
            .context("embedding provider was not resolved")?;
        let results = embed::rebuild_embeddings(&mut ctx.store, &ctx.project_id, provider, &depths)
            .context("re-embedding project memories")?;
        let targets: Vec<EmbedTarget> = results.into_iter().map(EmbedTarget::from).collect();
        let summary = build_summary(provider, targets, false);
        embed_summary = Some(summary);
    }

    let reindex_summary = ReindexSummary {
        fts_rebuilt,
        embeddings: embed_summary,
    };

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&reindex_summary),
        OutputFormat::Text => {
            print_reindex_text(&reindex_summary);
            Ok(())
        }
    }
}

// === PRIVATE HELPERS ===

fn print_reindex_text(summary: &ReindexSummary) {
    if summary.fts_rebuilt {
        println!("FTS5 index rebuilt.");
    }
    if let Some(ref es) = summary.embeddings {
        println!(
            "Embeddings reindexed: {} embedded, {} skipped, {} failed.",
            es.embedded.len(),
            es.skipped.len(),
            es.failed.len(),
        );
    }
}
