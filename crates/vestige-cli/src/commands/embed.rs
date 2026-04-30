//! `vestige embed` — generate and store embeddings for memory representations.
//!
//! Embeds `summary` and `compressed` representations for active memories by
//! default. Supports `--all` (entire project) or `--memory <id>` (single).
//! `--dry-run` prints targets without mutating the store.

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use time::OffsetDateTime;
use ulid::Ulid;
use vestige_core::{ListFilter, MemoryId, MemoryStatus, RepresentationDepth};
use vestige_store::{NewEmbedding, Store};

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

    let summary = if let Some(ref raw_id) = args.memory {
        embed_single(
            &mut ctx.store,
            raw_id,
            &ctx.project_id,
            &*provider,
            &depths,
            args.dry_run,
        )?
    } else {
        embed_all(
            &mut ctx.store,
            &ctx.project_id,
            &*provider,
            &depths,
            args.dry_run,
        )?
    };

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&summary),
        OutputFormat::Text => {
            print_summary_text(&summary);
            Ok(())
        }
    }
}

/// Embed every active memory in the project. Called by `embed --all` and
/// `reindex --embeddings` (the shared pipeline).
pub fn embed_all(
    store: &mut Store,
    project_id: &vestige_core::ProjectId,
    provider: &dyn vestige_embed::EmbeddingProvider,
    depths: &[RepresentationDepth],
    dry_run: bool,
) -> Result<EmbedSummary> {
    let memories = store
        .list_memories(project_id, &ListFilter::default())
        .context("listing memories")?;

    let mut targets: Vec<EmbedTarget> = Vec::new();

    for fetched in &memories {
        if fetched.memory.status != MemoryStatus::Active {
            continue;
        }
        let results = embed_memory_representations(store, fetched, provider, depths, dry_run)?;
        targets.extend(results);
    }

    Ok(build_summary(provider, targets, dry_run))
}

// === PRIVATE HELPERS ===

fn embed_single(
    store: &mut Store,
    raw_id: &str,
    _project_id: &vestige_core::ProjectId,
    provider: &dyn vestige_embed::EmbeddingProvider,
    depths: &[RepresentationDepth],
    dry_run: bool,
) -> Result<EmbedSummary> {
    use std::str::FromStr;
    let memory_id = MemoryId::from_str(raw_id)
        .with_context(|| format!("invalid memory id: {raw_id:?} — expected `mem_<ULID>`"))?;

    let fetched = store
        .get_memory(&memory_id)
        .context("fetching memory")?
        .with_context(|| format!("memory {raw_id} not found"))?;

    if fetched.memory.status != MemoryStatus::Active {
        anyhow::bail!(
            "memory {raw_id} is not active (status: {:?})",
            fetched.memory.status
        );
    }

    let targets = embed_memory_representations(store, &fetched, provider, depths, dry_run)?;
    Ok(build_summary(provider, targets, dry_run))
}

fn embed_memory_representations(
    store: &mut Store,
    fetched: &vestige_core::FetchedMemory,
    provider: &dyn vestige_embed::EmbeddingProvider,
    depths: &[RepresentationDepth],
    dry_run: bool,
) -> Result<Vec<EmbedTarget>> {
    let mut results = Vec::new();
    let memory_id = &fetched.memory.id;

    for &depth in depths {
        let repr = fetched.representations.iter().find(|r| r.depth == depth);

        let Some(repr) = repr else {
            results.push(EmbedTarget {
                memory_id: memory_id.as_str().to_owned(),
                representation_type: depth.as_str().to_owned(),
                action: EmbedAction::NoRepr,
                error: None,
            });
            continue;
        };

        // Look up the DB row id for this representation.
        let repr_db_id = fetch_repr_id(store, memory_id, depth)?;
        let Some(repr_id) = repr_db_id else {
            results.push(EmbedTarget {
                memory_id: memory_id.as_str().to_owned(),
                representation_type: depth.as_str().to_owned(),
                action: EmbedAction::NoRepr,
                error: None,
            });
            continue;
        };

        // Check whether an active, up-to-date embedding already exists.
        if !dry_run && has_current_embedding(store, &repr_id, provider)? {
            results.push(EmbedTarget {
                memory_id: memory_id.as_str().to_owned(),
                representation_type: depth.as_str().to_owned(),
                action: EmbedAction::Unchanged,
                error: None,
            });
            continue;
        }

        if dry_run {
            results.push(EmbedTarget {
                memory_id: memory_id.as_str().to_owned(),
                representation_type: depth.as_str().to_owned(),
                action: EmbedAction::WouldEmbed,
                error: None,
            });
            continue;
        }

        // Generate and persist the embedding.
        match provider.embed(&repr.content) {
            Ok(vector) => {
                let new_emb = NewEmbedding {
                    memory_id,
                    representation_id: &repr_id,
                    representation_type: depth.as_str(),
                    provider: provider.provider_name(),
                    model: provider.model_name(),
                    vector: &vector,
                };
                store
                    .record_embedding(&new_emb)
                    .context("recording embedding")?;
                results.push(EmbedTarget {
                    memory_id: memory_id.as_str().to_owned(),
                    representation_type: depth.as_str().to_owned(),
                    action: EmbedAction::Embedded,
                    error: None,
                });
            }
            Err(e) => {
                let error_msg = e.to_string();
                // Record a failed job row so `embeddings status` can surface it.
                let _ = record_failed_job(
                    store,
                    memory_id,
                    &repr_id,
                    depth.as_str(),
                    provider,
                    &error_msg,
                );
                results.push(EmbedTarget {
                    memory_id: memory_id.as_str().to_owned(),
                    representation_type: depth.as_str().to_owned(),
                    action: EmbedAction::Failed,
                    error: Some(error_msg),
                });
            }
        }
    }

    Ok(results)
}

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

/// Fetch the `memory_representations.id` column for a given (memory, depth) pair.
///
/// We drop into raw SQL here because `RepresentationRow` does not expose the DB
/// primary key — acceptable since this is an admin CLI operation.
fn fetch_repr_id(
    store: &Store,
    memory_id: &MemoryId,
    depth: RepresentationDepth,
) -> Result<Option<String>> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT id FROM memory_representations
             WHERE memory_id = ?1 AND representation_type = ?2",
        )
        .context("preparing repr id query")?;
    let mut rows = stmt
        .query(rusqlite::params![memory_id.as_str(), depth.as_str()])
        .context("querying repr id")?;
    if let Some(row) = rows.next().context("reading repr id row")? {
        Ok(Some(row.get(0).context("reading repr id column")?))
    } else {
        Ok(None)
    }
}

/// Return `true` if an active embedding already exists for this
/// `(representation_id, provider, model)` triple.
fn has_current_embedding(
    store: &Store,
    repr_id: &str,
    provider: &dyn vestige_embed::EmbeddingProvider,
) -> Result<bool> {
    let conn = store.connection();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memory_embeddings
             WHERE representation_id = ?1
               AND provider = ?2
               AND model = ?3
               AND status = 'active'",
            rusqlite::params![repr_id, provider.provider_name(), provider.model_name()],
            |r| r.get(0),
        )
        .context("checking existing embedding")?;
    Ok(count > 0)
}

/// Insert a failed `embedding_jobs` row so `embeddings status` can surface it.
fn record_failed_job(
    store: &mut Store,
    memory_id: &MemoryId,
    repr_id: &str,
    repr_type: &str,
    provider: &dyn vestige_embed::EmbeddingProvider,
    error: &str,
) -> Result<()> {
    let job_id = format!("job_{}", Ulid::new());
    let now_str = rfc3339_now()?;
    store
        .connection()
        .execute(
            "INSERT INTO embedding_jobs
                (id, memory_id, representation_id, representation_type,
                 provider, model, status, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'failed', ?7, ?8, ?8)",
            rusqlite::params![
                job_id,
                memory_id.as_str(),
                repr_id,
                repr_type,
                provider.provider_name(),
                provider.model_name(),
                error,
                now_str,
            ],
        )
        .context("recording failed embedding job")?;
    Ok(())
}

fn rfc3339_now() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting timestamp")
}

fn build_summary(
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
