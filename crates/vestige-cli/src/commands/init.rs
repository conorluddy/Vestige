use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use vestige_config::{
    build_init_config, discover_repo_root, display_name_from_path, git_remote_url, read_config,
    resolve_project_id, storage_path_for, write_config, CONFIG_DIR, CONFIG_FILE,
};
use vestige_store::Store;

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Human-readable project name. Also used to derive a stable project id.
    #[arg(long)]
    pub name: Option<String>,

    /// Optional one-line project summary stored as a `project_summary` memory.
    /// (Stored on first M1 milestone — accepted now and persisted in the
    /// init event payload so nothing is lost.)
    #[arg(long)]
    pub summary: Option<String>,

    /// Show the planned actions without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

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
        print_plan(&repo_root, &config_path, &storage_path, &cfg, args.summary.as_deref());
        return Ok(());
    }

    write_config(&config_path, &cfg).context("writing .vestige/config.toml")?;

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
        .record_event(&project_id, "project.initialised", Some(&payload.to_string()))
        .context("recording init event")?;

    println!("Initialised Vestige project `{project_name}` ({})", project_id.as_str());
    println!("  Config:   {}", config_path.display());
    println!("  Memory:   {}", storage_path.display());
    if let Some(s) = args.summary {
        println!("  Summary:  {s}");
        println!("  (Summary will be persisted as a project_summary memory in M1.)");
    }
    Ok(())
}

fn print_plan(
    repo_root: &PathBuf,
    config_path: &PathBuf,
    storage_path: &PathBuf,
    cfg: &vestige_config::VestigeConfig,
    summary: Option<&str>,
) {
    println!("[dry-run] would initialise Vestige in {}", repo_root.display());
    println!("  project_id:    {}", cfg.project_id);
    println!("  project_name:  {}", cfg.project_name);
    println!("  config:        {}", config_path.display());
    println!("  memory db:     {}", storage_path.display());
    if let Some(s) = summary {
        println!("  summary:       {s}");
    }
    println!("[dry-run] no files written.");
}
