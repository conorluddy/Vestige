//! `vestige scan` — one-shot session-log ingestion for the current project (V0.5.4).
//!
//! Mines this project's local Claude Code / Codex transcripts past their watermarks,
//! extracts candidates via the configured `[extraction]` provider, and files each into the
//! V0.2 inbox for review (never auto-promoted). `--dry-run` extracts and previews but writes
//! nothing — no candidates, no cursor movement.
//!
//! This is the non-daemon, non-agent entry point: it shares the exact same
//! [`vestige_engine::scan_and_propose`] core the daemon's `session_log_scan` job uses. Thin
//! adapter — no business logic here.

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

use vestige_engine::{scan_and_propose, ScanOptions, ScanPreview};

use crate::context;
use crate::output::emit_json;

// === CLI ARGS ===

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Extract and preview what would be proposed without writing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Override the extraction provider (`ollama`, `anthropic`, `openai`, `fake`).
    /// Defaults to the `[extraction]` section, then `ollama`.
    #[arg(long)]
    pub provider: Option<String>,

    /// Override the extraction model (provider-specific).
    #[arg(long)]
    pub model: Option<String>,

    #[arg(long)]
    pub json: bool,
}

// === OUTPUT SHAPE ===

#[derive(Debug, Serialize)]
struct ScanCliReport {
    sessions_scanned: usize,
    turns_processed: usize,
    candidates_proposed: usize,
    cursor_advanced: bool,
    dry_run: bool,
    previews: Vec<ScanCliPreview>,
}

#[derive(Debug, Serialize)]
struct ScanCliPreview {
    proposed_type: String,
    body: String,
    source_ref: String,
}

impl From<&ScanPreview> for ScanCliPreview {
    fn from(p: &ScanPreview) -> Self {
        ScanCliPreview {
            proposed_type: p.proposed_type.clone(),
            body: p.body.clone(),
            source_ref: p.source_ref.clone(),
        }
    }
}

// === PUBLIC API ===

pub fn run(args: ScanArgs) -> Result<()> {
    let mut ctx = context::load()?;

    // Resolve the extraction provider: CLI flags override the `[extraction]` section,
    // which itself defaults to `ollama`.
    let mut cfg = vestige_config::extraction_config_for(ctx.config.extraction.as_ref());
    if let Some(provider) = args.provider {
        cfg.provider = provider;
    }
    if args.model.is_some() {
        cfg.model = args.model;
    }

    let provider = vestige_extract::build_provider(&cfg).map_err(|e| {
        anyhow::anyhow!(
            "extraction provider `{}` unavailable: {e}\n\
             Rebuild with `--features extract-{}` (ollama/anthropic/openai), set a different \
             provider in [extraction], or use `--provider fake` for a dry test.",
            cfg.provider,
            cfg.provider
        )
    })?;

    let sources = vestige_engine::build_sources()
        .map_err(|e| anyhow::anyhow!("discovering session transcripts: {e}"))?;

    let report = scan_and_propose(
        &sources,
        &mut ctx.store,
        &ctx.project_id,
        provider.as_ref(),
        &ScanOptions {
            dry_run: args.dry_run,
            ..Default::default()
        },
    )
    .context("scanning session logs")?;

    let cli_report = ScanCliReport {
        sessions_scanned: report.sessions_scanned,
        turns_processed: report.turns_processed,
        candidates_proposed: report.candidates_proposed,
        cursor_advanced: report.cursor_advanced,
        dry_run: report.dry_run,
        previews: report.previews.iter().map(ScanCliPreview::from).collect(),
    };

    if args.json {
        emit_json(&cli_report)?;
    } else {
        print_text(&cli_report, provider.provider_name());
    }

    Ok(())
}

// === PRIVATE HELPERS ===

fn print_text(report: &ScanCliReport, provider_name: &str) {
    let verb = if report.dry_run {
        "would propose"
    } else {
        "proposed"
    };
    let count = if report.dry_run {
        report.previews.len()
    } else {
        report.candidates_proposed
    };

    println!(
        "scan ({provider}): {sessions} session(s), {turns} turn(s) — {verb} {count} candidate(s){dry}",
        provider = provider_name,
        sessions = report.sessions_scanned,
        turns = report.turns_processed,
        dry = if report.dry_run { " [dry-run, nothing written]" } else { "" },
    );

    for p in &report.previews {
        let body: String = p.body.chars().take(100).collect();
        println!("  • [{}] {}  ({})", p.proposed_type, body, p.source_ref);
    }

    if !report.dry_run && report.candidates_proposed > 0 {
        println!("\nReview with `vestige inbox`.");
    }
}
