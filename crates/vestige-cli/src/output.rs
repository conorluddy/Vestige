//! Output helpers — keep `--json` parity discipline in one place.
//!
//! Stdout is reserved for command output so `--json` consumers can parse it
//! cleanly. All logs go to stderr.

use anyhow::Result;
use serde::Serialize;
use vestige_core::{
    MemoryCard, MemoryDetail, MemoryStatus, RepresentationDepth, ScoredCard, SourceRow,
};

pub enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    pub fn pick(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Text
        }
    }
}

pub fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}

pub fn print_scored(scored: &ScoredCard) {
    let status_marker = match scored.card.status {
        MemoryStatus::Active => "",
        MemoryStatus::Deleted => " [deleted]",
    };
    println!(
        "{:<28} {:<14} {:>6.3}  {}{}",
        scored.card.id, scored.card.r#type, scored.score, scored.card.title, status_marker
    );
    if !scored.card.one_liner.is_empty() && scored.card.one_liner != scored.card.title {
        println!("    {}", scored.card.one_liner);
    }
}

pub fn print_card(card: &MemoryCard) {
    let status_marker = match card.status {
        MemoryStatus::Active => "",
        MemoryStatus::Deleted => " [deleted]",
    };
    println!(
        "{:<28} {:<14} {}{}",
        card.id, card.r#type, card.title, status_marker
    );
    if !card.one_liner.is_empty() && card.one_liner != card.title {
        println!("    {}", card.one_liner);
    }
}

pub fn print_detail(detail: &MemoryDetail, depth: RepresentationDepth, show_sources: bool) {
    let card = &detail.card;
    println!("{} ({})", card.id, card.r#type);
    println!("  status:     {:?}", card.status);
    println!("  importance: {}", card.importance);
    println!("  created:    {}", card.created_at);
    println!("  updated:    {}", card.updated_at);
    println!("  title:      {}", card.title);
    println!();
    println!("--- {} ---", depth.as_str());
    let content = detail
        .representations
        .iter()
        .find(|(d, _)| *d == depth)
        .map(|(_, c)| c.as_str())
        .unwrap_or("(missing)");
    println!("{content}");

    if show_sources && !detail.sources.is_empty() {
        println!();
        println!("--- sources ---");
        for src in &detail.sources {
            print_source(src);
        }
    }
}

fn print_source(src: &SourceRow) {
    let r = src.source_ref.as_deref().unwrap_or("-");
    println!("  [{}] {}", src.source_type, r);
    if let Some(content) = &src.source_content {
        for line in content.lines() {
            println!("    {line}");
        }
    }
}
