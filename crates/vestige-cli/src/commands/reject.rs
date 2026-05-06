//! `vestige reject` — dismiss a pending candidate with a reason.
//!
//! Thin dispatcher: parses `<cand_id>`, optional `--reason`, optional
//! `--duplicate-of`, and optional `--note`, then calls
//! [`vestige_engine::reject_candidate`]. No business logic here.

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{CandidateId, MemoryId, RejectionReason};
use vestige_engine::reject_candidate;

use crate::context;
use crate::output::{emit_json, OutputFormat};

/// Arguments for `vestige reject`.
#[derive(Debug, Args)]
pub struct RejectArgs {
    /// Candidate id to reject (cand_<ULID>).
    pub id: String,

    /// Rejection reason: duplicate | wrong | not_durable | too_noisy | stale | <freeform>.
    /// Defaults to `other:unspecified` if omitted.
    #[arg(long)]
    pub reason: Option<String>,

    /// Memory id this candidate duplicates (requires --reason duplicate).
    #[arg(long, value_name = "MEM_ID")]
    pub duplicate_of: Option<String>,

    /// Optional reviewer note.
    #[arg(long)]
    pub note: Option<String>,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: RejectArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let candidate_id = CandidateId::from_str(&args.id)?;

    let reason = args
        .reason
        .as_deref()
        .map(RejectionReason::from_str)
        .transpose()?
        .unwrap_or_else(|| RejectionReason::Other("unspecified".into()));

    let duplicate_of = args
        .duplicate_of
        .as_deref()
        .map(MemoryId::from_str)
        .transpose()?;

    reject_candidate(
        &mut ctx.store,
        &ctx.project_id,
        &candidate_id,
        reason.clone(),
        duplicate_of.clone(),
        args.note,
    )?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "candidate_id": candidate_id.to_string(),
            "status": "rejected",
            "reason": reason.as_str(),
            "duplicate_of": duplicate_of.as_ref().map(|id| id.to_string()),
        })),
        OutputFormat::Text => {
            if let Some(dup) = &duplicate_of {
                println!("Rejected {} → duplicate of {}", candidate_id, dup);
            } else {
                println!("Rejected {} (reason: {})", candidate_id, reason);
            }
            Ok(())
        }
    }
}
