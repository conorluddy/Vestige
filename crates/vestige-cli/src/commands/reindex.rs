//! `vestige reindex` — rebuild FTS and/or embedding indexes.
//!
//! Embeddings are a disposable acceleration layer; they can always be rebuilt
//! from the durable `memories` + `memory_representations` journal.
//!
//! `--fts`:        FTS5 `rebuild` command (SQLite shadow table reconstruction).
//! `--embeddings`: hard-delete all project embeddings, then re-embed everything.
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

    /// Delete all project embeddings and re-embed from scratch.
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

    let mut fts_rebuilt = false;
    let mut embed_summary: Option<EmbedSummary> = None;

    if do_fts {
        ctx.store
            .connection()
            .execute("INSERT INTO memory_fts(memory_fts) VALUES('rebuild')", [])
            .context("rebuilding FTS5 index")?;
        fts_rebuilt = true;
        tracing::info!("FTS5 index rebuilt");
    }

    if do_embeddings {
        // Hard-delete all embeddings for this project (vectors cascade via FK).
        // The filter is project-scoped per the hard rules.
        let deleted = ctx
            .store
            .connection()
            .execute(
                "DELETE FROM memory_embeddings
                 WHERE memory_id IN (
                     SELECT id FROM memories WHERE project_id = ?1
                 )",
                rusqlite::params![ctx.project_id.as_str()],
            )
            .context("clearing project embeddings")?;
        tracing::info!(deleted, "cleared embedding rows before reindex");

        let provider =
            context::embedding_provider(args.provider.as_deref(), args.model.as_deref(), None)?;

        let depths = vec![
            vestige_core::RepresentationDepth::Summary,
            vestige_core::RepresentationDepth::Compressed,
        ];

        let results = embed::embed_all(&mut ctx.store, &ctx.project_id, &*provider, &depths, false)
            .context("re-embedding project memories")?;
        let targets: Vec<EmbedTarget> = results.into_iter().map(EmbedTarget::from).collect();
        let summary = build_summary(&*provider, targets, false);
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
