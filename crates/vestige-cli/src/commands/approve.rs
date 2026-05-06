//! `vestige approve` — promote a pending candidate to a full memory.
//!
//! Thin dispatcher: parses `<cand_id>` and optional overrides, then calls
//! [`vestige_engine::approve_candidate`]. No business logic here.

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{CandidateId, MemoryType};
use vestige_engine::{approve_candidate, ApprovalOverrides};

use crate::context;
use crate::output::{emit_json, OutputFormat};

/// Arguments for `vestige approve`.
#[derive(Debug, Args)]
pub struct ApproveArgs {
    /// Candidate id to approve (cand_<ULID>).
    pub id: String,

    /// Override the proposed memory type.
    #[arg(long = "type", value_name = "TYPE")]
    pub proposed_type: Option<String>,

    /// Override the candidate body.
    #[arg(long)]
    pub body: Option<String>,

    /// Override importance in [0.0, 1.0].
    #[arg(long)]
    pub importance: Option<f32>,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ApproveArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let candidate_id = CandidateId::from_str(&args.id)?;

    let proposed_type = args
        .proposed_type
        .as_deref()
        .map(MemoryType::from_str)
        .transpose()?;

    let overrides = ApprovalOverrides {
        proposed_type,
        body: args.body,
        importance: args.importance,
    };

    let outcome = approve_candidate(&mut ctx.store, &ctx.project_id, &candidate_id, overrides)?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&serde_json::json!({
            "candidate_id": outcome.candidate_id.to_string(),
            "memory_id": outcome.memory_id.to_string(),
            "status": "approved",
        })),
        OutputFormat::Text => {
            println!("Approved {} → {}", outcome.candidate_id, outcome.memory_id);
            Ok(())
        }
    }
}
