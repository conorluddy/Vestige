//! `vestige remember` — free-form memory capture (type: note).
//!
//! Convenience alias for `vestige note add`. Accepts `--source`, `--source-content`,
//! and `--importance`. JSON output: `{ "id", "type", "truncated" }`.
//! Delegates to [`record::record`] via [`CaptureInput`].

use anyhow::Result;
use clap::Args;
use vestige_core::MemoryType;

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

    /// Emit JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// Record a free-form note and print the assigned memory ID.
pub fn run(args: RememberArgs) -> Result<()> {
    let mut ctx = context::load()?;
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: MemoryType::Note,
            body: &args.body,
            importance: args.importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
