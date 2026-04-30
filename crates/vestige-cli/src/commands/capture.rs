//! Shared add-args + dispatch for the four memory-capture commands
//! (decision / note / question / preference). They differ only in `MemoryType`,
//! the default `--importance`, and (decision only) optional rationale prefixing.

use anyhow::Result;
use clap::Args;
use vestige_core::MemoryType;

use crate::context;
use crate::output::OutputFormat;

use super::record::{record, CaptureInput};

#[derive(Debug, Clone, Copy)]
pub struct CaptureKind {
    pub memory_type: MemoryType,
    pub default_importance: f64,
    pub supports_rationale: bool,
}

pub const DECISION: CaptureKind = CaptureKind {
    memory_type: MemoryType::Decision,
    default_importance: 0.7,
    supports_rationale: true,
};
pub const NOTE: CaptureKind = CaptureKind {
    memory_type: MemoryType::Note,
    default_importance: 0.5,
    supports_rationale: false,
};
pub const QUESTION: CaptureKind = CaptureKind {
    memory_type: MemoryType::OpenQuestion,
    default_importance: 0.5,
    supports_rationale: false,
};
pub const PREFERENCE: CaptureKind = CaptureKind {
    memory_type: MemoryType::Preference,
    default_importance: 0.6,
    supports_rationale: false,
};

#[derive(Debug, Args)]
pub struct CaptureAddArgs {
    pub body: String,
    /// Decision-only: appended to the body as "Rationale: <text>".
    #[arg(long)]
    pub rationale: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub source_content: Option<String>,
    #[arg(long)]
    pub importance: Option<f64>,
    #[arg(long)]
    pub json: bool,
}

pub fn add(kind: CaptureKind, args: CaptureAddArgs) -> Result<()> {
    let mut ctx = context::load()?;
    let importance = args.importance.unwrap_or(kind.default_importance);
    let body = if kind.supports_rationale {
        match args.rationale.as_deref() {
            Some(r) => format!("{}\n\nRationale: {}", args.body, r),
            None => args.body.clone(),
        }
    } else {
        if args.rationale.is_some() {
            anyhow::bail!("--rationale is only supported for `decision`");
        }
        args.body.clone()
    };
    record(
        &mut ctx.store,
        &ctx.project_id,
        CaptureInput {
            r#type: kind.memory_type,
            body: &body,
            importance,
            source_ref: args.source.as_deref(),
            source_content: args.source_content.as_deref(),
        },
        OutputFormat::pick(args.json),
    )
}
