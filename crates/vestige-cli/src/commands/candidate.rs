//! `vestige candidate` — propose a new candidate to the assimilation inbox.
//!
//! Thin dispatcher: `vestige candidate add --type <type> --body <body> [opts]`
//! builds a [`NewCandidate`] and calls [`vestige_engine::propose_candidate`].

use std::str::FromStr;

use anyhow::Result;
use clap::{Args, Subcommand};
use vestige_core::{CandidateId, MemoryId, MemoryType, NewCandidate, NewCandidateSource};
use vestige_engine::propose_candidate;

use crate::context;
use crate::output::{emit_json, OutputFormat};

/// Arguments for `vestige candidate`.
#[derive(Debug, Args)]
pub struct CandidateArgs {
    #[command(subcommand)]
    pub command: CandidateCommand,
}

/// Subcommands for `vestige candidate`.
#[derive(Debug, Subcommand)]
pub enum CandidateCommand {
    /// Propose a new candidate to the assimilation inbox.
    Add(CandidateAddArgs),
}

/// Arguments for `vestige candidate add`.
#[derive(Debug, Args)]
pub struct CandidateAddArgs {
    /// Proposed memory type: decision, note, observation, preference, project_summary, open_question.
    #[arg(long = "type", value_name = "TYPE")]
    pub proposed_type: String,

    /// Full body of the candidate (required).
    #[arg(long)]
    pub body: String,

    /// Optional title override (falls back to derived title if omitted).
    #[arg(long)]
    pub title: Option<String>,

    /// Why this observation is worth recording.
    #[arg(long)]
    pub rationale: Option<String>,

    /// Signal strength in [0.0, 1.0].
    #[arg(long, default_value_t = 0.5)]
    pub importance: f32,

    /// Agent confidence in [0.0, 1.0].
    #[arg(long, default_value_t = 0.5)]
    pub confidence: f32,

    /// Source category (e.g. file, url, clipboard).
    #[arg(long)]
    pub source_type: Option<String>,

    /// Source locator (file path, URL, etc.).
    #[arg(long)]
    pub source_ref: Option<String>,

    /// Source content snippet (truncated to 2 KiB by core).
    #[arg(long)]
    pub source_content: Option<String>,

    /// Duplicate of an existing memory (`mem_<ULID>`).
    #[arg(long, value_name = "MEM_ID")]
    pub duplicate_of_memory: Option<String>,

    /// Duplicate of another pending candidate (`cand_<ULID>`).
    #[arg(long, value_name = "CAND_ID")]
    pub duplicate_of_candidate: Option<String>,

    #[arg(long)]
    pub json: bool,
}

pub fn run(args: CandidateArgs) -> Result<()> {
    match args.command {
        CandidateCommand::Add(a) => add(a),
    }
}

fn add(args: CandidateAddArgs) -> Result<()> {
    let mut ctx = context::load()?;

    let proposed_type = MemoryType::from_str(&args.proposed_type)?;

    let duplicate_of_memory_id = args
        .duplicate_of_memory
        .as_deref()
        .map(MemoryId::from_str)
        .transpose()?;

    let duplicate_of_candidate_id = args
        .duplicate_of_candidate
        .as_deref()
        .map(CandidateId::from_str)
        .transpose()?;

    let source = match args.source_type {
        Some(source_type) => Some(NewCandidateSource {
            source_type,
            source_ref: args.source_ref,
            source_content: args.source_content,
        }),
        None => None,
    };

    let new_candidate = NewCandidate {
        project_id: ctx.project_id.clone(),
        proposed_type,
        body: args.body,
        rationale: args.rationale,
        title_override: args.title,
        importance: args.importance,
        confidence: args.confidence,
        source,
        duplicate_of_memory_id,
        duplicate_of_candidate_id,
    };

    let outcome = propose_candidate(&mut ctx.store, &ctx.project_id, new_candidate)?;

    match OutputFormat::pick(args.json) {
        OutputFormat::Json => {
            #[derive(serde::Serialize)]
            struct SimilarMemoryJson {
                id: String,
                title: String,
                score: f32,
            }
            #[derive(serde::Serialize)]
            struct SimilarCandidateJson {
                id: String,
                title: String,
                score: f32,
            }
            #[derive(serde::Serialize)]
            struct ProposeJson {
                candidate_id: String,
                status: String,
                similar_memories: Vec<SimilarMemoryJson>,
                similar_candidates: Vec<SimilarCandidateJson>,
            }
            emit_json(&ProposeJson {
                candidate_id: outcome.candidate_id.to_string(),
                status: outcome.status.as_str().to_string(),
                similar_memories: outcome
                    .similar_memories
                    .iter()
                    .map(|m| SimilarMemoryJson {
                        id: m.id.to_string(),
                        title: m.title.clone(),
                        score: m.score,
                    })
                    .collect(),
                similar_candidates: outcome
                    .similar_candidates
                    .iter()
                    .map(|c| SimilarCandidateJson {
                        id: c.id.to_string(),
                        title: c.title.clone(),
                        score: c.score,
                    })
                    .collect(),
            })
        }
        OutputFormat::Text => {
            println!(
                "Proposed {} {}",
                proposed_type.as_str(),
                outcome.candidate_id
            );
            if !outcome.similar_memories.is_empty() {
                let list: Vec<String> = outcome
                    .similar_memories
                    .iter()
                    .map(|m| format!("{} ({:.2})", m.id, m.score))
                    .collect();
                println!("  Similar memories: {}", list.join(", "));
            }
            if !outcome.similar_candidates.is_empty() {
                let list: Vec<String> = outcome
                    .similar_candidates
                    .iter()
                    .map(|c| format!("{} ({:.2})", c.id, c.score))
                    .collect();
                println!("  Similar pending: {}", list.join(", "));
            }
            Ok(())
        }
    }
}
