//! `vestige embeddings` — subcommand parent for embedding management.
//!
//! Current subcommands:
//!   - `status` — show embedding coverage for the active project.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::context;
use crate::output::{emit_json, OutputFormat};

// === CLI ARGS ===

#[derive(Debug, Args)]
pub struct EmbeddingsArgs {
    #[command(subcommand)]
    pub command: EmbeddingsCommand,
}

#[derive(Debug, Subcommand)]
pub enum EmbeddingsCommand {
    /// Show embedding coverage and index state for the active project.
    Status(EmbeddingsStatusArgs),
}

#[derive(Debug, Args)]
pub struct EmbeddingsStatusArgs {
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: EmbeddingsArgs) -> Result<()> {
    match args.command {
        EmbeddingsCommand::Status(a) => status(a),
    }
}

// === PRIVATE HELPERS ===

fn status(args: EmbeddingsStatusArgs) -> Result<()> {
    let ctx = context::load()?;
    let es = ctx
        .store
        .embedding_status(&ctx.project_id)
        .map_err(anyhow::Error::from)?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => {
            // `EmbeddingStatus` is not `Serialize` — build a json! literal.
            let value = json!({
                "project_id":                es.project_id.as_str(),
                "provider":                  es.provider,
                "model":                     es.model,
                "dimensions":                es.dimensions,
                "total_active_memories":     es.total_active_memories,
                "embeddable_representations":es.embeddable_representations,
                "embedded_representations":  es.embedded_representations,
                "stale_embeddings":          es.stale_embeddings,
                "failed_jobs":               es.failed_jobs,
                "missing_embeddings":        es.missing_embeddings,
            });
            emit_json(&value)
        }
        OutputFormat::Text => {
            println!("Project:   {}", ctx.config.project_name);
            println!("Provider:  {}", es.provider.as_deref().unwrap_or("(none)"));
            println!("Model:     {}", es.model.as_deref().unwrap_or("(none)"));
            println!(
                "Dimensions:{}",
                es.dimensions
                    .map(|d| format!(" {d}"))
                    .unwrap_or_else(|| " (none)".to_string())
            );
            println!();
            println!(
                "Memories:                    {} active",
                es.total_active_memories
            );
            println!(
                "Embeddable representations:  {}",
                es.embeddable_representations
            );
            println!(
                "Embedded representations:    {}",
                es.embedded_representations
            );
            println!("Stale embeddings:            {}", es.stale_embeddings);
            println!("Failed jobs:                 {}", es.failed_jobs);
            println!("Missing embeddings:          {}", es.missing_embeddings);
            Ok(())
        }
    }
}
