//! `vestige inbox` — list and inspect the assimilation inbox.
//!
//! Default invocation (`vestige inbox`) lists pending candidates. Subcommand
//! `show <cand_id>` renders a full candidate detail view.

use std::str::FromStr;

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use vestige_core::{CandidateId, MemoryType};
use vestige_store::CandidateFilter;

use crate::context;
use crate::output::{emit_json, OutputFormat};

/// Arguments for `vestige inbox`.
#[derive(Debug, Args)]
pub struct InboxArgs {
    #[command(subcommand)]
    pub command: Option<InboxCommand>,

    /// Maximum number of candidates to list.
    #[arg(long)]
    pub limit: Option<u32>,

    /// Filter by proposed type.
    #[arg(long = "type", value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Include rejected candidates in the list.
    #[arg(long)]
    pub include_rejected: bool,

    #[arg(long)]
    pub json: bool,
}

/// Subcommands for `vestige inbox`.
#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// Show full detail for a candidate.
    Show(InboxShowArgs),
}

/// Arguments for `vestige inbox show`.
#[derive(Debug, Args)]
pub struct InboxShowArgs {
    /// Candidate id (cand_<ULID>).
    pub id: String,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: InboxArgs) -> Result<()> {
    match args.command {
        Some(InboxCommand::Show(show_args)) => run_show(show_args),
        None => run_list(args),
    }
}

fn run_list(args: InboxArgs) -> Result<()> {
    let ctx = context::load()?;

    let r#type = args
        .r#type
        .as_deref()
        .map(<MemoryType as std::str::FromStr>::from_str)
        .transpose()?;

    let filter = CandidateFilter {
        status: None,
        proposed_type: r#type,
        limit: args.limit,
        include_rejected: args.include_rejected,
    };

    let candidates = ctx.store.list_candidates(&ctx.project_id, &filter)?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => {
            #[derive(serde::Serialize)]
            struct CandidateListJson {
                id: String,
                r#type: String,
                status: String,
                title: String,
                one_liner: String,
                confidence: f32,
                importance: f32,
                similar_memories: Vec<serde_json::Value>,
                created_at: String,
            }
            #[derive(serde::Serialize)]
            struct Envelope {
                candidates: Vec<CandidateListJson>,
            }
            let items: Vec<CandidateListJson> = candidates
                .iter()
                .map(|c| CandidateListJson {
                    id: c.id.to_string(),
                    r#type: c.proposed_type.as_str().to_string(),
                    status: c.status.as_str().to_string(),
                    title: c.title.clone(),
                    one_liner: c.one_liner.clone(),
                    confidence: c.confidence,
                    importance: c.importance,
                    similar_memories: vec![],
                    created_at: c.created_at.to_string(),
                })
                .collect();
            emit_json(&Envelope { candidates: items })
        }
        OutputFormat::Text => {
            let header = if args.include_rejected {
                "Candidates (pending + rejected)"
            } else {
                "Pending candidates"
            };
            println!("{header}: {}", candidates.len());
            println!();
            for c in &candidates {
                let id_short = &c.id.as_str()[..c.id.as_str().len().min(20)];
                let type_padded = format!("{:<10}", c.proposed_type.as_str());
                let one_liner = if c.one_liner.len() > 60 {
                    format!("{}…", &c.one_liner[..59])
                } else {
                    c.one_liner.clone()
                };
                println!(
                    "{:<20}  {}  {:.2}  {}",
                    id_short, type_padded, c.confidence, one_liner
                );
                let similar = c
                    .duplicate_of_memory_id
                    .as_ref()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "none".to_string());
                println!("             Similar: {similar}");
            }
            Ok(())
        }
    }
}

fn run_show(args: InboxShowArgs) -> Result<()> {
    let ctx = context::load()?;
    let candidate_id = CandidateId::from_str(&args.id)?;

    let candidate = ctx
        .store
        .get_candidate(&candidate_id)?
        .ok_or_else(|| anyhow!("candidate not found: {}", args.id))?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => emit_json(&candidate),
        OutputFormat::Text => {
            println!("{} ({})", candidate.id, candidate.proposed_type.as_str());
            println!("  status:     {}", candidate.status.as_str());
            println!("  confidence: {:.2}", candidate.confidence);
            println!("  importance: {:.2}", candidate.importance);
            println!("  title:      {}", candidate.title);
            println!("  one_liner:  {}", candidate.one_liner);
            println!("  created:    {}", candidate.created_at);
            if let Some(rationale) = &candidate.rationale {
                println!();
                println!("--- rationale ---");
                println!("{rationale}");
            }
            println!();
            println!("--- body ---");
            println!("{}", candidate.full_body);
            if !candidate.sources.is_empty() {
                println!();
                println!("--- sources ---");
                for src in &candidate.sources {
                    let r = src.source_ref.as_deref().unwrap_or("-");
                    println!("  [{}] {}", src.source_type, r);
                    if let Some(content) = &src.source_content {
                        for line in content.lines() {
                            println!("    {line}");
                        }
                    }
                }
            }
            if let Some(dup_mem) = &candidate.duplicate_of_memory_id {
                println!();
                println!("  Duplicate of memory: {dup_mem}");
            }
            if let Some(dup_cand) = &candidate.duplicate_of_candidate_id {
                println!();
                println!("  Duplicate of candidate: {dup_cand}");
            }
            match candidate.status {
                vestige_core::CandidateStatus::Pending => {
                    println!();
                    println!("  Approve: vestige approve {}", candidate.id);
                    println!(
                        "  Reject:  vestige reject {} --reason <reason>",
                        candidate.id
                    );
                }
                vestige_core::CandidateStatus::Approved => {
                    println!();
                    if let Some(mem_id) = &candidate.approved_memory_id {
                        println!("  Approved as: {mem_id}");
                    }
                    if let Some(reviewed) = &candidate.reviewed_at {
                        println!("  Reviewed at: {reviewed}");
                    }
                }
                vestige_core::CandidateStatus::Rejected => {
                    println!();
                    if let Some(reason) = &candidate.rejection_reason {
                        println!("  Rejection reason: {}", reason.as_str());
                    }
                    if let Some(note) = &candidate.review_note {
                        println!("  Review note: {note}");
                    }
                    if let Some(reviewed) = &candidate.reviewed_at {
                        println!("  Reviewed at: {reviewed}");
                    }
                }
                vestige_core::CandidateStatus::Superseded => {
                    println!();
                    println!("  Superseded.");
                }
            }
            Ok(())
        }
    }
}
