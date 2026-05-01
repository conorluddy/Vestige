//! `vestige init` — pin Vestige memory to the current repository.
//!
//! Creates `.vestige/config.toml` and opens (or migrates) the per-project
//! SQLite store at `~/.vestige/projects/<project_id>/memory.sqlite`.
//! **Idempotent**: re-running never rotates `project_id` or duplicates the
//! project row. `--dry-run` prints the planned actions without writing anything.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Args;

use vestige_config::{
    build_init_config, discover_repo_root, display_name_from_path, git_remote_url, read_config,
    resolve_project_id, storage_path_for, write_config, CONFIG_DIR, CONFIG_FILE,
};
use vestige_core::{build_bundle, ListFilter, MemoryType, NewMemory};
use vestige_store::Store;

/// Arguments for `vestige init`.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Human-readable project name. Also used to derive a stable project id.
    #[arg(long)]
    pub name: Option<String>,

    /// Optional one-line project summary persisted as a `project_summary`
    /// memory. Idempotent: re-running `init` won't duplicate it.
    #[arg(long)]
    pub summary: Option<String>,

    /// Show the planned actions without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Initialise (or re-confirm) Vestige for the current repository.
pub fn run(args: InitArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let repo_root = discover_repo_root(&cwd);
    let config_path = repo_root.join(CONFIG_DIR).join(CONFIG_FILE);

    // Idempotency: if a config already exists, reuse its project id rather
    // than re-deriving (so a `--name` change later doesn't rotate ids).
    let existing = if config_path.is_file() {
        Some(read_config(&config_path)?)
    } else {
        None
    };

    let project_id = match &existing {
        Some(cfg) => cfg.project_id()?,
        None => resolve_project_id(&repo_root, args.name.as_deref()),
    };
    let project_name = args
        .name
        .clone()
        .or_else(|| existing.as_ref().map(|c| c.project_name.clone()))
        .unwrap_or_else(|| display_name_from_path(&repo_root));

    let storage_path = storage_path_for(&project_id)?;
    let cfg = build_init_config(&project_id, &project_name, &storage_path);

    if args.dry_run {
        print_plan(
            &repo_root,
            &config_path,
            &storage_path,
            &cfg,
            args.summary.as_deref(),
            existing.is_some(),
        );
        return Ok(());
    }

    // First write wins. Once `.vestige/config.toml` exists the file belongs to
    // the user — re-running `init` must not stomp on hand edits or comments.
    if existing.is_none() {
        write_config(&config_path, &cfg).context("writing .vestige/config.toml")?;
    }

    let mut store = Store::open(&storage_path).context("opening project store")?;
    store
        .ensure_project(
            &project_id,
            &project_name,
            Some(repo_root.to_string_lossy().as_ref()),
            git_remote_url(&repo_root).as_deref(),
        )
        .context("registering project row")?;

    let payload = serde_json::json!({
        "project_id": project_id.as_str(),
        "name": project_name,
        "summary": args.summary,
        "repo_root": repo_root.to_string_lossy(),
    });
    store
        .record_event(
            &project_id,
            "project.initialised",
            Some(&payload.to_string()),
        )
        .context("recording init event")?;

    if let Some(summary) = args.summary.as_deref() {
        if !summary_already_recorded(&store, &project_id)? {
            let bundle = build_bundle(
                &project_id,
                NewMemory {
                    r#type: MemoryType::ProjectSummary,
                    body: summary,
                    importance: 0.9,
                    source: None,
                },
            )?;
            store.record_memory(&bundle)?;
        }
    }

    let is_fresh = existing.is_none();
    let banner = if is_fresh {
        "Initialised"
    } else {
        "Already initialised"
    };
    println!(
        "{banner} Vestige project `{project_name}` ({})",
        project_id.as_str()
    );
    println!("  Config:   {}", config_path.display());
    println!("  Memory:   {}", storage_path.display());
    if let Some(s) = args.summary {
        println!("  Summary:  {s}");
    }

    if is_fresh {
        print_next_steps();
    }
    Ok(())
}

/// Print onboarding hints after a first-time `init`. Skipped on re-runs so
/// the success line stays compact for the agent-driven happy path.
///
/// Today: just the MCP wiring step. When skills are published, add a second
/// bullet pointing at the install flow (a one-liner here).
fn print_next_steps() {
    println!();
    println!("Next steps:");
    println!("  Wire Vestige into Claude Code as an MCP server:");
    println!("    claude mcp add vestige -s project -- vestige mcp");
    println!();
    println!("  Capture your first memory:");
    println!("    vestige decision add \"…\" --rationale \"…\"");
    println!("    vestige note add \"…\"");
    println!();
    println!("  Inspect state:  vestige status");
}

/// Return `true` if a `project_summary` memory already exists (so we don't duplicate).
fn summary_already_recorded(
    store: &Store,
    project_id: &vestige_core::ProjectId,
) -> anyhow::Result<bool> {
    let existing = store.list_memories(
        project_id,
        &ListFilter {
            include_deleted: false,
            r#type: Some(MemoryType::ProjectSummary),
            limit: Some(1),
        },
    )?;
    Ok(!existing.is_empty())
}

/// Print the dry-run plan without writing any files.
fn print_plan(
    repo_root: &Path,
    config_path: &Path,
    storage_path: &Path,
    cfg: &vestige_config::VestigeConfig,
    summary: Option<&str>,
    config_exists: bool,
) {
    println!(
        "[dry-run] would initialise Vestige in {}",
        repo_root.display()
    );
    println!("  project_id:    {}", cfg.project_id);
    println!("  project_name:  {}", cfg.project_name);
    if config_exists {
        println!(
            "  config:        {} (exists; would not rewrite)",
            config_path.display()
        );
    } else {
        println!("  config:        {}", config_path.display());
    }
    println!("  memory db:     {}", storage_path.display());
    if let Some(s) = summary {
        println!("  summary:       {s}");
    }
    println!("[dry-run] no files written.");
}
