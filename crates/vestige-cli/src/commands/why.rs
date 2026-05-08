//! `vestige why <mem_or_cand_id>` — templated provenance walk for a memory or candidate.
//!
//! Thin adapter: parse the ID prefix → dispatch to `vestige-engine::walk_provenance` →
//! render text or JSON. No business logic lives here.

use anyhow::{Context, Result};
use clap::Args;
use vestige_engine::{walk_provenance, SubjectId};

use crate::context;
use crate::output::emit_json;

/// Arguments for `vestige why`.
#[derive(Debug, Args)]
pub struct WhyArgs {
    /// Memory or candidate id to trace (`mem_<ULID>` or `cand_<ULID>`).
    pub id: String,

    /// Output JSON (matches PRD §13.1 shape).
    #[arg(long)]
    pub json: bool,

    /// Include full source content inline in text output.
    #[arg(long)]
    pub depth: Option<String>,
}

pub fn run(args: WhyArgs) -> Result<()> {
    let ctx = context::load()?;

    let subject =
        SubjectId::parse(&args.id).with_context(|| format!("invalid id `{}`", args.id))?;

    let walk = walk_provenance(&ctx.store, &ctx.project_id, &subject)
        .with_context(|| format!("provenance walk for `{}`", args.id))?;

    let include_full = args.depth.as_deref() == Some("full");

    if args.json {
        emit_json(&walk)
    } else {
        print_walk(&walk, include_full);
        Ok(())
    }
}

// === TEXT RENDERING ===

fn print_walk(walk: &vestige_engine::ProvenanceWalk, include_full: bool) {
    // Header line.
    let id_str = walk
        .memory_id
        .as_deref()
        .or(walk.candidate_id.as_deref())
        .unwrap_or("?");

    println!("{id_str}  {}  status={}", walk.subject_type, walk.status);

    // Provenance events.
    println!();
    println!("Provenance walk:");

    let events = walk
        .provenance
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if events.is_empty() {
        println!("  (no events recorded)");
    } else {
        for e in &events {
            let evt_type = e["type"].as_str().unwrap_or("?");
            let at = e["at"].as_str().unwrap_or("?");
            let evt_id = e["event_id"].as_str().unwrap_or("?");
            // Format timestamp to date+time without subseconds.
            let at_display = at.get(..19).unwrap_or(at);
            println!("  ◇ {evt_type:<28}  {at_display}  ({evt_id})");
        }
    }

    // Candidate back-reference.
    if let Some(cand) =
        walk.provenance
            .get("candidate")
            .and_then(|v| if v.is_null() { None } else { Some(v) })
    {
        if let Some(cand_id) = cand["candidate_id"].as_str() {
            println!("  ◆ Promoted from candidate {cand_id}");
            if let Some(cand_events) = cand["events"].as_array() {
                for e in cand_events {
                    let evt_type = e["type"].as_str().unwrap_or("?");
                    let at = e["at"].as_str().unwrap_or("?");
                    let evt_id = e["event_id"].as_str().unwrap_or("?");
                    let at_display = at.get(..19).unwrap_or(at);
                    println!("    ◇ {evt_type:<28}  {at_display}  ({evt_id})");
                }
            }
        }
    }

    // Sources.
    let sources = walk
        .provenance
        .get("sources")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    println!();
    println!("Sources ({}):", sources.len());

    if sources.is_empty() {
        println!("  (none)");
    } else {
        for src in &sources {
            let kind = src["kind"].as_str().unwrap_or("?");
            let src_id = src["id"].as_str().unwrap_or("?");
            let ref_str = src["source_ref"].as_str().unwrap_or("-");
            println!("  ─ {src_id}  {kind:<14}  ref={ref_str}");

            if include_full {
                if let Some(content) = src["content"].as_str() {
                    for line in content.lines().take(5) {
                        println!("    {line}");
                    }
                    if src["content"]
                        .as_str()
                        .map(|c| c.lines().count())
                        .unwrap_or(0)
                        > 5
                    {
                        println!("    …");
                    }
                }
            }
        }
    }

    // Status history (mirrors events, printed compactly).
    println!();
    println!("Status history:");
    for e in &walk.status_history {
        let at_display = e.at.get(..19).unwrap_or(&e.at);
        println!("  {at_display}  {}", e.event_type);
    }
    if walk.status_history.is_empty() {
        println!("  (no status transitions recorded)");
    }
}
