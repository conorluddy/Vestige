//! `vestige skills` — install or list skills bundled into this binary.
//!
//! `install` copies the bundled `.claude/skills/` snapshot into a repo,
//! defaulting to `<repo-root>/.claude/skills/`. Hard-fails on drift unless
//! `--force` is set. `list` enumerates bundled skills with their descriptions.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

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
    /// Install bundled skills into a repo's `.claude/skills/` directory.
    Install(InstallArgs),
    /// List skills bundled into this `vestige` binary.
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Destination directory. Defaults to <repo-root>/.claude/skills/.
    #[arg(long)]
    pub dest: Option<PathBuf>,
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

// === PUBLIC API ===

pub fn run(args: SkillsArgs) -> Result<()> {
    match args.action {
        SkillsAction::Install(install_args) => run_install(install_args),
        SkillsAction::List(list_args) => run_list(list_args),
    }
}

// === PRIVATE HELPERS ===

fn run_install(args: InstallArgs) -> Result<()> {
    let dest = match args.dest {
        Some(path) => path,
        None => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            vestige_config::discover_repo_root(&cwd).join(".claude/skills")
        }
    };

    let report = bundle::install(&dest, args.force, args.dry_run)
        .with_context(|| format!("installing skills into {}", dest.display()))?;

    if !report.drifted.is_empty() {
        match OutputFormat::pick(args.json) {
            OutputFormat::Json => {
                emit_json(&report)?;
            }
            OutputFormat::Text => {
                eprintln!("error: {} file(s) have local edits:", report.drifted.len());
                for path in &report.drifted {
                    eprintln!("  drifted: {path}");
                }
                eprintln!("Re-run with --force to overwrite.");
                print_install_summary(&report);
            }
        }
        return Err(anyhow!(
            "{} skill file(s) have local edits; use --force to overwrite",
            report.drifted.len()
        ));
    }

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&report),
        OutputFormat::Text => {
            print_install_summary(&report);
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

fn print_install_summary(report: &bundle::InstallReport) {
    let prefix = if report.dry_run { "[dry-run] " } else { "" };
    println!(
        "{}Installed {} skill file(s) (skipped {}, drifted {}) → {}",
        prefix,
        report.written.len(),
        report.skipped.len(),
        report.drifted.len(),
        report.dest.display()
    );
}
