//! `vestige embed` — generate and store embeddings for memory representations.
//!
//! Embeds `summary` and `compressed` representations for active memories by
//! default. Supports `--all` (entire project) or `--memory <id>` (single).
//! `--dry-run` prints targets without mutating the store.

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::str::FromStr;
use vestige_core::{MemoryId, MemoryStatus, RepresentationDepth};
use vestige_engine::embed::{self, EmbedOutcome};

use crate::context;
use crate::output::{emit_json, OutputFormat};

// === TYPES ===

/// Which action would be (or was) taken for a single (memory, representation) target.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbedAction {
    /// Vector was generated and persisted.
    Embedded,
    /// An active, current embedding already exists — skipped.
    Unchanged,
    /// Memory has no representation of this type — skipped.
    NoRepr,
    /// Would embed (dry-run only).
    WouldEmbed,
    /// Embedding the representation failed; a failed job row was recorded.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbedTarget {
    pub memory_id: String,
    pub representation_type: String,
    pub action: EmbedAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EmbedSummary {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub embedded: Vec<EmbedTarget>,
    pub skipped: Vec<EmbedTarget>,
    pub failed: Vec<EmbedTarget>,
    pub dry_run: bool,
}

impl From<embed::EmbedResult> for EmbedTarget {
    fn from(r: embed::EmbedResult) -> Self {
        let action = match r.outcome {
            EmbedOutcome::Embedded => EmbedAction::Embedded,
            EmbedOutcome::Unchanged => EmbedAction::Unchanged,
            EmbedOutcome::NoRepr => EmbedAction::NoRepr,
            EmbedOutcome::WouldEmbed => EmbedAction::WouldEmbed,
            EmbedOutcome::Failed => EmbedAction::Failed,
        };
        EmbedTarget {
            memory_id: r.memory_id.as_str().to_owned(),
            representation_type: r.representation_type,
            action,
            error: r.error,
        }
    }
}

// === CLI ARGS ===

#[derive(Debug, Args)]
pub struct EmbedArgs {
    /// Embed all active project memories.
    #[arg(long, conflicts_with = "memory")]
    pub all: bool,

    /// Embed a single memory by its ID.
    #[arg(long, value_name = "MEMORY_ID", conflicts_with = "all")]
    pub memory: Option<String>,

    /// Representation depths to embed. Defaults to `summary` and `compressed`.
    /// Pass multiple times: `--representation summary --representation compressed`.
    #[arg(long = "representation", value_name = "DEPTH")]
    pub representations: Vec<String>,

    /// Override the embedding provider (e.g. `fake`, `fastembed`, `ollama`).
    #[arg(long)]
    pub provider: Option<String>,

    /// Override the model name (provider-specific).
    #[arg(long)]
    pub model: Option<String>,

    /// Print what would be embedded without writing to the store.
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: EmbedArgs) -> Result<()> {
    if !args.all && args.memory.is_none() {
        anyhow::bail!("one of --all or --memory <MEMORY_ID> is required");
    }

    let mut ctx = context::load()?;

    let provider = context::embedding_provider(
        args.provider.as_deref(),
        args.model.as_deref(),
        None, // dimensions from provider default
    )?;

    let depths = resolve_depths(&args.representations)?;

    let results = if let Some(ref raw_id) = args.memory {
        let memory_id = MemoryId::from_str(raw_id)
            .with_context(|| format!("invalid memory id: {raw_id:?} — expected `mem_<ULID>`"))?;
        let fetched = ctx
            .store
            .get_memory(&memory_id)
            .context("fetching memory")?
            .with_context(|| format!("memory {raw_id} not found"))?;
        if fetched.memory.status != MemoryStatus::Active {
            anyhow::bail!(
                "memory {raw_id} is not active (status: {:?})",
                fetched.memory.status
            );
        }
        embed::embed_memory_representations(
            &mut ctx.store,
            &fetched,
            &*provider,
            &depths,
            args.dry_run,
        )
        .context("embedding memory representations")?
    } else {
        embed::embed_all(
            &mut ctx.store,
            &ctx.project_id,
            &*provider,
            &depths,
            args.dry_run,
        )
        .context("embedding all memories")?
    };

    let targets: Vec<EmbedTarget> = results.into_iter().map(EmbedTarget::from).collect();
    let summary = build_summary(&*provider, targets, args.dry_run);

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&summary),
        OutputFormat::Text => {
            print_summary_text(&summary);
            Ok(())
        }
    }
}

// === PRIVATE HELPERS ===

/// Resolve `--representation` flags (or default to `["summary", "compressed"]`).
fn resolve_depths(raw: &[String]) -> Result<Vec<RepresentationDepth>> {
    if raw.is_empty() {
        return Ok(vec![
            RepresentationDepth::Summary,
            RepresentationDepth::Compressed,
        ]);
    }
    raw.iter()
        .map(|s| match s.as_str() {
            "summary" => Ok(RepresentationDepth::Summary),
            "compressed" => Ok(RepresentationDepth::Compressed),
            "full" => Ok(RepresentationDepth::Full),
            "one_liner" => Ok(RepresentationDepth::OneLiner),
            other => anyhow::bail!(
                "unknown representation depth {other:?} — valid values: summary, compressed, full, one_liner"
            ),
        })
        .collect()
}

pub(crate) fn build_summary(
    provider: &dyn vestige_embed::EmbeddingProvider,
    targets: Vec<EmbedTarget>,
    dry_run: bool,
) -> EmbedSummary {
    let mut embedded = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();

    for t in targets {
        match t.action {
            EmbedAction::Embedded | EmbedAction::WouldEmbed => embedded.push(t),
            EmbedAction::Failed => failed.push(t),
            EmbedAction::Unchanged | EmbedAction::NoRepr => skipped.push(t),
        }
    }

    EmbedSummary {
        provider: provider.provider_name().to_owned(),
        model: provider.model_name().to_owned(),
        dimensions: provider.dimensions(),
        embedded,
        skipped,
        failed,
        dry_run,
    }
}

fn print_summary_text(summary: &EmbedSummary) {
    let verb = if summary.dry_run {
        "Would embed"
    } else {
        "Embedded"
    };
    println!(
        "{} {} representations across {} memories using provider={} model={}",
        verb,
        summary.embedded.len(),
        unique_memories(&summary.embedded),
        summary.provider,
        summary.model,
    );
    if summary.dry_run {
        for t in &summary.embedded {
            println!("  would_embed  {} ({})", t.memory_id, t.representation_type);
        }
        for t in &summary.skipped {
            let label = match t.action {
                EmbedAction::Unchanged => "unchanged",
                EmbedAction::NoRepr => "no_repr",
                _ => "skip",
            };
            println!("  {}  {} ({})", label, t.memory_id, t.representation_type);
        }
    }
    println!(
        "Embedded {}; skipped {}; failed {}.",
        summary.embedded.len(),
        summary.skipped.len(),
        summary.failed.len(),
    );
    for t in &summary.failed {
        eprintln!(
            "  FAILED {} ({}): {}",
            t.memory_id,
            t.representation_type,
            t.error.as_deref().unwrap_or("unknown error"),
        );
    }
}

fn unique_memories(targets: &[EmbedTarget]) -> usize {
    use std::collections::HashSet;
    targets
        .iter()
        .map(|t| &t.memory_id)
        .collect::<HashSet<_>>()
        .len()
}
