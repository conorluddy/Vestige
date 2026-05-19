//! `vestige status` — show the active project state.
//!
//! Prints the project name, ID, scope, config and memory-DB paths, and a
//! brief count of active vs. deleted memories. Optionally surfaces daemon
//! health when `~/.vestige/daemon.status.json` is present.

use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;

use vestige_config::discover_config;
use vestige_daemon::ipc::status_file;
use vestige_store::Store;

// === TYPES ===

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

/// Print a project status overview to stdout.
pub fn run(args: StatusArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let (config_path, cfg) = discover_config(&cwd).context(
        "no Vestige project found from this directory — run `vestige init` to create one",
    )?;

    let project_id = cfg.project_id()?;
    let storage_path = cfg.resolved_storage_path()?;
    let store = Store::open(&storage_path).context("opening project store")?;
    let counts = store.memory_counts(&project_id)?;

    let repo_root = config_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    let daemon_status = read_daemon_status_best_effort();

    if args.json {
        let daemon_block = build_daemon_json(&daemon_status, &project_id);
        let output = json!({
            "project": {
                "id": project_id.as_str(),
                "name": cfg.project_name,
                "scope": cfg.scope.to_string(),
                "repo_root": repo_root.display().to_string(),
                "config": config_path.display().to_string(),
                "memory_db": storage_path.display().to_string(),
            },
            "memories": {
                "active": counts.active,
                "deleted": counts.deleted,
            },
            "daemon": daemon_block,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Project:    {} ({})", cfg.project_name, cfg.project_id);
        println!("Scope:      {}", cfg.scope);
        println!("Repo root:  {}", repo_root.display());
        println!("Config:     {}", config_path.display());
        println!("Memory DB:  {}", storage_path.display());
        println!(
            "Memories:   {} active, {} deleted",
            counts.active, counts.deleted
        );
        println!("MCP:        run `vestige mcp` to expose this project to an agent over stdio");

        print_daemon_text(&daemon_status, &project_id);
    }

    Ok(())
}

// === PRIVATE HELPERS ===

/// Read the daemon status file, treating any error as "not running".
///
/// Best-effort: logs I/O or parse errors at `debug` level and returns `None`
/// so `vestige status` never fails because of a daemon issue.
fn read_daemon_status_best_effort() -> Option<vestige_daemon::ipc::status_file::DaemonStatus> {
    let path = status_file::resolve_status_path(None);
    match status_file::read(&path) {
        Ok(maybe) => maybe,
        Err(err) => {
            tracing::debug!("could not read daemon status file: {err}");
            None
        }
    }
}

/// Format the daemon block for text output.
fn print_daemon_text(
    daemon: &Option<vestige_daemon::ipc::status_file::DaemonStatus>,
    project_id: &vestige_core::ProjectId,
) {
    match daemon {
        None => {
            println!("Daemon:     not running (no status file at ~/.vestige/daemon.status.json)");
        }
        Some(s) => {
            println!(
                "Daemon:     running  pid={}  uptime={}s  version={}",
                s.pid, s.uptime_secs, s.version
            );
            println!("            projects supervised: {}", s.projects.len());
            match s.projects.iter().find(|p| p.project_id == *project_id) {
                Some(ps) => {
                    let last_embed = ps.last_embed_run.as_deref().unwrap_or("never");
                    println!(
                        "            this project: last_embed={}  pending_embeds={}",
                        last_embed, ps.pending_embeds
                    );
                }
                None => {
                    println!(
                        "            this project: not yet supervised (run `vestige daemon kick embed` to register)"
                    );
                }
            }
        }
    }
}

/// Build the `daemon` JSON block for `--json` output.
fn build_daemon_json(
    daemon: &Option<vestige_daemon::ipc::status_file::DaemonStatus>,
    project_id: &vestige_core::ProjectId,
) -> serde_json::Value {
    match daemon {
        None => json!({ "running": false }),
        Some(s) => {
            let this_project = match s.projects.iter().find(|p| p.project_id == *project_id) {
                Some(ps) => json!({
                    "supervised": true,
                    "last_embed_run": ps.last_embed_run,
                    "pending_embeds": ps.pending_embeds,
                }),
                None => json!({ "supervised": false }),
            };
            json!({
                "running": true,
                "pid": s.pid,
                "uptime_secs": s.uptime_secs,
                "version": s.version,
                "projects_supervised": s.projects.len(),
                "this_project": this_project,
            })
        }
    }
}
