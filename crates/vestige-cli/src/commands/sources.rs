//! `vestige sources <mem_or_cand_id>` — list typed source receipts for a memory or candidate.
//!
//! Thin adapter: parse ID prefix → dispatch to `vestige-engine::list_sources` →
//! render tabular text or JSON. No business logic lives here.

use anyhow::{Context, Result};
use clap::Args;
use vestige_engine::{list_sources, SourceReceipt, SubjectId};

use crate::context;
use crate::output::emit_json;

/// Arguments for `vestige sources`.
#[derive(Debug, Args)]
pub struct SourcesArgs {
    /// Memory or candidate id (`mem_<ULID>` or `cand_<ULID>`).
    pub id: String,

    /// Filter by source kind (file, commit, url, agent_session, mcp_call, candidate, manual, trace).
    #[arg(long)]
    pub kind: Option<String>,

    /// Output JSON (matches PRD §13.2 shape).
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: SourcesArgs) -> Result<()> {
    let ctx = context::load()?;

    let subject =
        SubjectId::parse(&args.id).with_context(|| format!("invalid id `{}`", args.id))?;

    let listing = list_sources(&ctx.store, &ctx.project_id, &subject, args.kind.as_deref())
        .with_context(|| format!("listing sources for `{}`", args.id))?;

    if args.json {
        emit_json(&listing)
    } else {
        print_listing(&listing);
        Ok(())
    }
}

// === TEXT RENDERING ===

fn print_listing(listing: &vestige_engine::SourceListing) {
    println!("{}  {} sources", listing.owner_id, listing.sources.len());

    if listing.sources.is_empty() {
        println!("(no sources attached)");
        return;
    }

    println!();
    for src in &listing.sources {
        print_source(src);
    }
}

fn print_source(src: &SourceReceipt) {
    let ref_str = src.source_ref.as_deref().unwrap_or("-");
    println!("{}  {:<14}  {}", src.id, src.kind, ref_str);

    if let Some(content) = &src.content {
        // Print the first few lines with indentation.
        for line in content.lines().take(4) {
            println!("    {line}");
        }
        if content.lines().count() > 4 {
            println!("    …");
        }
        let size_kib = content.len() as f64 / 1024.0;
        let cap_kib = vestige_core::SOURCE_SNIPPET_MAX_BYTES as f64 / 1024.0;
        println!(
            "    (truncated: {}; {:.1} KiB / {:.1} KiB)",
            src.truncated, size_kib, cap_kib
        );
    }
}
