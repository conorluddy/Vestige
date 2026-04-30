//! Output helpers — keep `--json` parity discipline in one place.
//!
//! Stdout is reserved for command output so `--json` consumers can parse it
//! cleanly. All logs go to stderr.

use anyhow::Result;
use serde::Serialize;
use vestige_core::{
    MemoryCard, MemoryDetail, MemoryStatus, RepresentationDepth, ScoredCard, SearchMode, SourceRow,
};

/// Selects between human-readable text and machine-parseable JSON output.
///
/// Every command that accepts `--json` constructs this via [`OutputFormat::pick`]
/// and branches on it. Stdout is reserved for structured output; warnings and
/// logs always go to stderr.
pub enum OutputFormat {
    /// Human-readable text, suitable for terminal display.
    Text,
    /// Pretty-printed JSON, suitable for scripting and agent consumers.
    Json,
}

impl OutputFormat {
    /// Select `Json` when `json` is `true`, otherwise `Text`.
    pub fn pick(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Text
        }
    }
}

/// Serialise `value` to pretty-printed JSON and write it to stdout.
pub fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}

/// Emit the search result envelope per PRD §12.6.
///
/// ```json
/// { "mode": "hybrid", "results": [...], "warnings": [...] }
/// ```
pub fn emit_search_json(
    mode: SearchMode,
    results: &[ScoredCard],
    warnings: &[String],
) -> Result<()> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        mode: &'a str,
        results: &'a [ScoredCard],
        warnings: &'a [String],
    }
    emit_json(&Envelope {
        mode: mode.as_str(),
        results,
        warnings,
    })
}

/// Print a compact scored card without score-parts breakdown.
#[allow(dead_code)] // kept for stability; PR4 commands may import this
pub fn print_scored(scored: &ScoredCard) {
    print_scored_opts(scored, false);
}

/// Print a compact scored card. When `include_parts` is true and `score_parts`
/// is populated, a second line with the breakdown is printed.
pub fn print_scored_opts(scored: &ScoredCard, include_parts: bool) {
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
    if include_parts {
        if let Some(parts) = &scored.score_parts {
            println!(
                "    [fts={:.3} vec={:.3} imp={:.3} type={:.3}]",
                parts.fts, parts.vector, parts.importance, parts.type_boost
            );
        }
    }
}

/// Print a compact memory card: `<id>  <type>  <title>  [deleted]`.
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

/// Print a memory at the requested representation depth, optionally including
/// attached source rows. Used by `vestige show`.
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

/// Print a single source attachment row (`[type] ref\n  content`).
fn print_source(src: &SourceRow) {
    let r = src.source_ref.as_deref().unwrap_or("-");
    println!("  [{}] {}", src.source_type, r);
    if let Some(content) = &src.source_content {
        for line in content.lines() {
            println!("    {line}");
        }
    }
}
