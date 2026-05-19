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

use crate::commands::skills::{resolve_targets, Target as SkillsTarget};
use crate::output::{emit_json, OutputFormat};

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

    /// Emit a structured JSON envelope (project_id, name, db_path, fresh).
    #[arg(long)]
    pub json: bool,

    /// Skip the bundled-skills install. Default behaviour is to write every
    /// shipped SKILL.md into <repo>/.claude/skills/ AND <repo>/.agents/skills/
    /// during init.
    #[arg(long)]
    pub no_install_skills: bool,

    /// Which agent-skills directory(ies) to install to. Default `both`
    /// covers Claude Code (`.claude/skills/`) and the agentskills.io standard
    /// (`.agents/skills/`).
    #[arg(long, value_enum, default_value_t = SkillsTarget::Both)]
    pub skills_target: SkillsTarget,
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
        if args.json {
            return emit_json(&serde_json::json!({
                "dry_run": true,
                "project_id": cfg.project_id,
                "name": cfg.project_name,
                "config_path": config_path.to_string_lossy(),
                "db_path": storage_path.to_string_lossy(),
                "summary": args.summary,
                "fresh": existing.is_none(),
                "skills_installed": null,
            }));
        }
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

    // Install bundled skills into the selected target dir(s) — best-effort,
    // never fails init. Drift is warned about but not treated as an error;
    // the user can run `vestige skills install --force` to overwrite.
    let skills_results = if args.no_install_skills {
        None
    } else {
        let targets = resolve_targets(None, args.skills_target, &repo_root);
        let mut results = Vec::with_capacity(targets.len());
        for (label, dest) in targets {
            match crate::skills::bundle::install(&dest, false, false) {
                Ok(report) => {
                    if !report.drifted.is_empty() {
                        tracing::warn!(
                            project_id = %project_id,
                            target = label,
                            dest = %dest.display(),
                            drifted = ?report.drifted,
                            "skills install: local edits detected; run `vestige skills install --force` to overwrite"
                        );
                    }
                    results.push((label, report));
                }
                Err(err) => {
                    tracing::warn!(
                        project_id = %project_id,
                        target = label,
                        error = %err,
                        dest = %dest.display(),
                        "skills install failed; init succeeded"
                    );
                }
            }
        }
        Some(results)
    };

    register_with_daemon_best_effort(project_id.as_str(), &project_name, &repo_root);

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => {
            let skills_installed = skills_results.as_ref().map(|results| {
                let entries: Vec<serde_json::Value> = results
                    .iter()
                    .map(|(label, r)| {
                        serde_json::json!({
                            "target": label,
                            "written": r.written.len(),
                            "skipped": r.skipped.len(),
                            "drifted": r.drifted.len(),
                            "dest": r.dest.to_string_lossy(),
                        })
                    })
                    .collect();
                serde_json::json!({ "results": entries })
            });
            emit_json(&serde_json::json!({
                "project_id": project_id.as_str(),
                "name": project_name,
                "config_path": config_path.to_string_lossy(),
                "db_path": storage_path.to_string_lossy(),
                "summary": args.summary,
                "fresh": is_fresh,
                "skills_installed": skills_installed,
            }))
        }
        OutputFormat::Text => {
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
            if let Some(s) = &args.summary {
                println!("  Summary:  {s}");
            }
            if let Some(results) = &skills_results {
                for (label, report) in results {
                    print_skills_line(label, report);
                }
            }
            if is_fresh {
                print_next_steps();
            }
            Ok(())
        }
    }
}

/// Print onboarding hints after a first-time `init`. Skipped on re-runs so
/// the success line stays compact for the agent-driven happy path.
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
    println!("  Inspect installed skills:");
    println!("    vestige skills list");
    println!();
    println!("  Inspect state:  vestige status");
}

/// Print the one-line skills install summary for text-mode output.
///
/// Three variants per target:
/// - All up to date → "Already up to date (N files)"
/// - Some written, no drift → "Installed N skill files"
/// - Drift detected → counts + escape-hatch hint
fn print_skills_line(label: &str, report: &crate::skills::bundle::InstallReport) {
    let written = report.written.len();
    let skipped = report.skipped.len();
    let drifted = report.drifted.len();
    let dest = report.dest.display();

    if written == 0 && drifted == 0 {
        println!("  Skills [{label}]: Already up to date ({skipped} files) → {dest}");
    } else if drifted == 0 {
        println!("  Skills [{label}]: Installed {written} skill files → {dest}");
    } else {
        println!(
            "  Skills [{label}]: Installed {written} (skipped {skipped}, drifted {drifted}) → {dest}"
        );
        println!("            run `vestige skills install --force` to overwrite local edits");
    }
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

/// Best-effort: tell the running daemon about this new project so it gets
/// supervised immediately. If no daemon is running, return silently — this
/// MUST NEVER fail `vestige init`.
fn register_with_daemon_best_effort(
    project_id: &str,
    project_name: &str,
    repo_root: &std::path::Path,
) {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    if is_ephemeral_repo_root(repo_root) {
        tracing::debug!(
            repo_root = %repo_root.display(),
            "ephemeral repo root; skipping daemon registration to avoid leaking test projects"
        );
        return;
    }

    let socket = vestige_daemon::ipc::server::resolve_socket_path(None);
    if !socket.exists() {
        tracing::debug!(socket = %socket.display(), "daemon socket not present; skipping registration");
        return;
    }

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "daemon.register_project",
        "params": {
            "project_id": project_id,
            "project_name": project_name,
            "repo_root": repo_root.display().to_string(),
        }
    });

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::debug!(error = %e, "could not build tokio runtime for daemon ping");
            return;
        }
    };

    let outcome = runtime.block_on(async {
        let connect =
            tokio::time::timeout(Duration::from_millis(500), UnixStream::connect(&socket)).await;
        let stream = match connect {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(format!("connect: {e}")),
            Err(_) => return Err("connect timeout".to_string()),
        };
        let (_read, mut write) = tokio::io::split(stream);
        let payload = format!("{}\n", req);
        match tokio::time::timeout(
            Duration::from_millis(500),
            write.write_all(payload.as_bytes()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("write: {e}")),
            Err(_) => return Err("write timeout".to_string()),
        }
        // Fire-and-forget: don't bother reading the response.
        Ok(())
    });

    match outcome {
        Ok(()) => tracing::debug!(project_id, "registered with running daemon"),
        Err(e) => tracing::debug!(error = %e, "daemon registration skipped"),
    }
}

/// Return `true` when `repo_root` looks like a throwaway test directory.
///
/// We've been leaking `~/.vestige/projects/proj_*` entries because integration
/// tests `vestige init` inside a `tempfile::TempDir`, the daemon registers the
/// project, then the TempDir is removed without telling the daemon — leaving
/// orphan entries in the menu-bar app forever. Bail before the IPC ping when
/// the repo root is obviously ephemeral.
fn is_ephemeral_repo_root(repo_root: &std::path::Path) -> bool {
    if std::env::var_os("VESTIGE_TEST").is_some() {
        return true;
    }

    let canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    const EPHEMERAL_PREFIXES: &[&str] = &[
        "/tmp",
        "/private/tmp",
        "/var/folders",
        "/private/var/folders",
    ];
    for prefix in EPHEMERAL_PREFIXES {
        if canonical.starts_with(prefix) {
            return true;
        }
    }

    if let Ok(system_tmp) = std::env::temp_dir().canonicalize() {
        if canonical.starts_with(&system_tmp) {
            return true;
        }
    }

    false
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

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    /// `register_with_daemon_best_effort` must never panic or error when no
    /// daemon socket is present. Uses a TempDir as HOME so the default socket
    /// path resolves to an empty directory where no socket file can exist.
    #[test]
    fn register_with_daemon_silently_succeeds_when_no_socket() {
        let tmp = tempfile::TempDir::new().unwrap();
        let original_home = std::env::var_os("HOME");
        // Point HOME at the TempDir so resolve_socket_path returns a path
        // inside it — no socket file will exist there.
        std::env::set_var("HOME", tmp.path());
        register_with_daemon_best_effort("proj_test", "test", tmp.path());
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }
        // No assertion needed beyond "didn't panic".
    }

    #[test]
    fn ephemeral_check_flags_tempdir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(
            is_ephemeral_repo_root(tmp.path()),
            "tempfile::TempDir paths must be classified as ephemeral; got {}",
            tmp.path().display()
        );
    }

    #[test]
    fn ephemeral_check_flags_known_prefixes() {
        for prefix in [
            "/tmp/foo",
            "/private/tmp/foo",
            "/var/folders/x/y",
            "/private/var/folders/x/y",
        ] {
            assert!(
                is_ephemeral_repo_root(std::path::Path::new(prefix)),
                "expected {prefix} to be ephemeral"
            );
        }
    }

    #[test]
    fn ephemeral_check_skips_real_repo_paths() {
        // A path that doesn't canonicalize and doesn't start with any temp
        // prefix should be treated as a real repo.
        let path = std::path::Path::new("/Users/someone/Development/MyProject");
        std::env::remove_var("VESTIGE_TEST");
        assert!(!is_ephemeral_repo_root(path));
    }

    #[test]
    fn ephemeral_check_honours_env_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("VESTIGE_TEST", "1");
        let real_looking = std::path::Path::new("/Users/x/y");
        assert!(is_ephemeral_repo_root(real_looking));
        std::env::remove_var("VESTIGE_TEST");
        // Tempdir still ephemeral after env removal.
        assert!(is_ephemeral_repo_root(tmp.path()));
    }
}
