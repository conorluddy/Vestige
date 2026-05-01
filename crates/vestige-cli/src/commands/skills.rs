//! `vestige skills` — install or list skills bundled into this binary.
//!
//! `install` copies the bundled skill snapshot into a repo, defaulting to
//! BOTH `<repo>/.claude/skills/` (Claude Code) and `<repo>/.agents/skills/`
//! (the agentskills.io open standard, read by Codex and other compliant
//! tools). The `--target` flag narrows to a single dir; `--dest <path>`
//! overrides the resolution entirely. Hard-fails on drift unless `--force`.
//! `list` enumerates bundled skills with their descriptions.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand, ValueEnum};

use crate::output::{emit_json, OutputFormat};
use crate::skills::bundle;

// === TYPES ===

#[derive(Debug, Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub action: SkillsAction,
}

#[derive(Debug, Subcommand)]
pub enum SkillsAction {
    /// Install bundled skills into a repo's agent-skills directory.
    Install(InstallArgs),
    /// List skills bundled into this `vestige` binary.
    List(ListArgs),
}

/// Which agent-skills directory(ies) to write to.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Target {
    /// Only `<repo>/.claude/skills/` (Claude Code).
    Claude,
    /// Only `<repo>/.agents/skills/` (agentskills.io / Codex).
    Agents,
    /// Both `.claude/skills/` and `.agents/skills/` — the default.
    Both,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Install to this exact directory. Overrides `--target`.
    #[arg(long)]
    pub dest: Option<PathBuf>,
    /// Which agent-skills directory to install to.
    #[arg(long, value_enum, default_value_t = Target::Both)]
    pub target: Target,
    /// Overwrite locally edited skill files.
    #[arg(long)]
    pub force: bool,
    /// Show actions without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Emit a structured JSON envelope.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Emit a structured JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// One install attempt against a single target directory, plus the label
/// (`claude` / `agents` / `custom`) used to identify it in JSON output.
#[derive(Debug, serde::Serialize)]
struct LabelledReport {
    target: &'static str,
    #[serde(flatten)]
    report: bundle::InstallReport,
}

// === PUBLIC API ===

pub fn run(args: SkillsArgs) -> Result<()> {
    match args.action {
        SkillsAction::Install(install_args) => run_install(install_args),
        SkillsAction::List(list_args) => run_list(list_args),
    }
}

/// Resolve the user's `--dest`/`--target` choice into the list of
/// `(label, dest)` pairs the install loop should iterate over.
///
/// Public so `init` can reuse the resolver and emit the same labelled
/// envelope shape.
pub fn resolve_targets(
    explicit_dest: Option<&Path>,
    target: Target,
    repo_root: &Path,
) -> Vec<(&'static str, PathBuf)> {
    if let Some(dest) = explicit_dest {
        return vec![("custom", dest.to_path_buf())];
    }
    match target {
        Target::Claude => vec![("claude", repo_root.join(".claude").join("skills"))],
        Target::Agents => vec![("agents", repo_root.join(".agents").join("skills"))],
        Target::Both => vec![
            ("claude", repo_root.join(".claude").join("skills")),
            ("agents", repo_root.join(".agents").join("skills")),
        ],
    }
}

// === PRIVATE HELPERS ===

fn run_install(args: InstallArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let repo_root = vestige_config::discover_repo_root(&cwd);
    let targets = resolve_targets(args.dest.as_deref(), args.target, &repo_root);

    let mut reports: Vec<LabelledReport> = Vec::with_capacity(targets.len());
    for (label, dest) in &targets {
        let report = bundle::install(dest, args.force, args.dry_run)
            .with_context(|| format!("installing skills into {}", dest.display()))?;
        reports.push(LabelledReport {
            target: label,
            report,
        });
    }

    let drift_total: usize = reports.iter().map(|r| r.report.drifted.len()).sum();

    if drift_total > 0 {
        match OutputFormat::pick(args.json) {
            OutputFormat::Json => {
                emit_json(&serde_json::json!({
                    "dry_run": args.dry_run,
                    "results": reports,
                }))?;
            }
            OutputFormat::Text => {
                eprintln!("error: {drift_total} file(s) have local edits:");
                for r in &reports {
                    for path in &r.report.drifted {
                        eprintln!("  drifted ({}): {path}", r.target);
                    }
                }
                eprintln!("Re-run with --force to overwrite.");
                for r in &reports {
                    print_install_summary(r);
                }
            }
        }
        return Err(anyhow!(
            "{drift_total} skill file(s) have local edits; use --force to overwrite"
        ));
    }

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "dry_run": args.dry_run,
            "results": reports,
        })),
        OutputFormat::Text => {
            for r in &reports {
                print_install_summary(r);
            }
            Ok(())
        }
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    let skills = bundle::list();

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "skills": skills,
        })),
        OutputFormat::Text => {
            if skills.is_empty() {
                println!("(no bundled skills)");
                return Ok(());
            }
            let name_width = skills
                .iter()
                .map(|s| s.name.len())
                .max()
                .unwrap_or(0)
                .max(4);
            println!(
                "{:<width$}  {:>5}  description",
                "name",
                "files",
                width = name_width
            );
            println!("{}", "-".repeat(name_width + 2 + 5 + 2 + 11));
            for skill in &skills {
                println!(
                    "{:<width$}  {:>5}  {}",
                    skill.name,
                    skill.files,
                    skill.description,
                    width = name_width
                );
            }
            Ok(())
        }
    }
}

fn print_install_summary(r: &LabelledReport) {
    let prefix = if r.report.dry_run { "[dry-run] " } else { "" };
    println!(
        "{}[{}] Installed {} skill file(s) (skipped {}, drifted {}) → {}",
        prefix,
        r.target,
        r.report.written.len(),
        r.report.skipped.len(),
        r.report.drifted.len(),
        r.report.dest.display()
    );
}
