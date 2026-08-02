//! `vestige remember` — free-form memory capture (type: note).
//!
//! Convenience alias for `vestige note add`. Accepts `--source`, `--source-content`,
//! `--importance`, and `--supersedes <mem_id>` (issue #131). JSON output:
//! `{ "id", "type", "truncated", "supersedes"? }`.
//! Delegates to `super::record::record` via `CaptureInput`.

use std::str::FromStr;

use anyhow::Result;
use clap::Args;
use vestige_core::{MemoryId, MemoryType};

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

/// Arguments for `vestige remember`.
#[derive(Debug, Args)]
pub struct RememberArgs {
    /// The memory body. Captured as a `note` by default.
    pub body: String,

    /// Optional source reference (file path, URL, etc.).
    #[arg(long)]
    pub source: Option<String>,

    /// Optional inline source snippet (capped at 2 KiB).
    #[arg(long, value_name = "TEXT")]
    pub source_content: Option<String>,

    /// Importance in [0.0, 1.0]. Default 0.5.
    #[arg(long, default_value_t = 0.5)]
    pub importance: f64,

    /// Soft-delete and link the given memory as superseded by this one.
    #[arg(long, value_name = "MEM_ID")]
    pub supersedes: Option<String>,

    /// Emit JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// Record a free-form note and print the assigned memory ID.
pub fn run(args: RememberArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let supersedes = args
        .supersedes
        .as_deref()
        .map(MemoryId::from_str)
        .transpose()?;
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::Note,
            body: &args.body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
            supersedes: supersedes.as_ref(),
        },
        OutputFormat::pick(args.json),
    )
}
