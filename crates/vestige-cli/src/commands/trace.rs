//! `vestige trace` — list recent query traces or inspect one in detail.
//!
//! Dispatch is determined by the presence of a `<trace_id>` or named subcommand:
//!
//! - `vestige trace` — list mode (default: last 10 traces)
//! - `vestige trace <trace_id>` — show one trace in detail
//! - `vestige trace replay <trace_id>` — reserved for M5; slot is defined here
//!
//! `--kind`, `--caller`, `--since`, `--limit`, and `--json` are list-mode flags
//! but are accepted globally for forward compatibility and ignored in show mode.
//!
//! # Clap structure
//!
//! We use an optional positional `trace_or_sub` arg to capture either a bare
//! `trace_<ULID>` (→ show mode) or the literal string `"replay"` (→ M5
//! subcommand). This avoids registering a named `show` subcommand when the
//! PRD surface is just `vestige trace <id>`.
//!
//! No business logic lives here — format-and-dispatch only; all query logic
//! lives in `vestige-engine::trace_read`.

use std::str::FromStr;

use anyhow::{Context, Result};
use clap::Args;
use vestige_core::TraceId;
use vestige_engine::{
    get_trace, list_traces, replay_trace, ListFilters, ReplayResult, TraceCard, TraceDetail,
};

use crate::context;
use crate::output::emit_json;

// === CLAP TYPES ===

/// Arguments for `vestige trace`.
#[derive(Debug, Args)]
pub struct TraceArgs {
    /// Trace ID to show (`trace_<ULID>`), or the literal `replay` for M5.
    ///
    /// When absent the command runs in list mode.
    pub trace_id: Option<String>,

    /// M5 placeholder: the trace ID that follows `vestige trace replay <id>`.
    ///
    /// Currently unused — M5 will activate this field.
    #[arg(hide = true)]
    pub replay_id: Option<String>,

    /// Maximum number of traces to return (list mode only).
    #[arg(long, default_value = "10")]
    pub limit: u32,

    /// Filter by trace kind: search | expand | context (list mode only).
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by caller: cli | mcp (list mode only).
    #[arg(long)]
    pub caller: Option<String>,

    /// Only traces at or after this date/datetime — ISO-8601 or RFC-3339 (list mode only).
    #[arg(long)]
    pub since: Option<String>,

    /// Output JSON matching PRD §13.3 shapes.
    #[arg(long)]
    pub json: bool,
}

// === DISPATCH ===

pub fn run(args: TraceArgs) -> Result<()> {
    match args.trace_id.as_deref() {
        None => run_list(&args),
        Some("replay") => {
            let replay_id = args
                .replay_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("usage: vestige trace replay <trace_id>"))?;
            run_replay(replay_id, args.json)
        }
        Some(id) => run_show(id, args.json),
    }
}

// === LIST ===

fn run_list(args: &TraceArgs) -> Result<()> {
    let ctx = context::load()?;

    let filters = ListFilters {
        kind: args.kind.as_deref(),
        caller: args.caller.as_deref(),
        since: args.since.as_deref(),
        limit: args.limit,
    };

    let traces =
        list_traces(&ctx.store, &ctx.project_id, &filters).map_err(|e| anyhow::anyhow!("{e}"))?;

    if args.json {
        #[derive(serde::Serialize)]
        struct ListEnvelope<'a> {
            traces: &'a [TraceCard],
        }
        emit_json(&ListEnvelope { traces: &traces })
    } else {
        print_list(&traces);
        Ok(())
    }
}

// === REPLAY ===

fn run_replay(trace_id_str: &str, json: bool) -> Result<()> {
    use vestige_embed::build_provider;
    use vestige_engine::trace::Caller;

    let trace_id = TraceId::from_str(trace_id_str)
        .with_context(|| format!("invalid trace id `{trace_id_str}` — expected `trace_<ULID>`"))?;

    let ctx = context::load()?;

    // Try to build the configured embedding provider. A missing or disabled
    // provider is not an error here — replay falls back to lexical and surfaces
    // `provider_match = false` in the output.
    let provider_box: Option<Box<dyn vestige_embed::EmbeddingProvider>> =
        build_provider(&ctx.resolve_embeddings_config()).ok();
    let provider_ref: Option<&dyn vestige_embed::EmbeddingProvider> = provider_box.as_deref();

    let result = replay_trace(
        &ctx.store,
        provider_ref,
        &ctx.project_id,
        &trace_id,
        Caller::Cli,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        emit_json(&result)
    } else {
        print_replay(&result);
        Ok(())
    }
}

// === SHOW ===

fn run_show(trace_id_str: &str, json: bool) -> Result<()> {
    let trace_id = TraceId::from_str(trace_id_str)
        .with_context(|| format!("invalid trace id `{trace_id_str}` — expected `trace_<ULID>`"))?;

    let ctx = context::load()?;

    let detail =
        get_trace(&ctx.store, &ctx.project_id, &trace_id).map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        emit_json(&detail)
    } else {
        print_detail(&detail);
        Ok(())
    }
}

// === TEXT RENDERING — LIST ===

fn print_list(traces: &[TraceCard]) {
    if traces.is_empty() {
        println!("No query traces recorded yet.");
        return;
    }

    println!("Recent query traces ({} shown):", traces.len());
    println!();

    for t in traces {
        let mode_str = t.mode.as_deref().unwrap_or("—");
        let query_display = t
            .query
            .as_deref()
            .map(truncate_query)
            .unwrap_or_else(|| "—".to_string());

        // Trim timestamp to date+time (drop sub-seconds and timezone).
        let ts = t.created_at.get(..19).unwrap_or(&t.created_at);

        println!(
            "{:<28}  {}  {:<9}  {:<9}  {:<32}  {:>3} results  {}ms  caller={}",
            t.trace_id, ts, t.kind, mode_str, query_display, t.result_count, t.latency_ms, t.caller,
        );
    }
}

/// Truncate a query string to at most 30 display characters, appending `…`
/// when shortened. Truncation is at a char boundary to avoid breaking UTF-8.
fn truncate_query(q: &str) -> String {
    const MAX: usize = 30;
    let mut chars = q.chars();
    let preview: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

// === TEXT RENDERING — SHOW ===

fn print_detail(d: &TraceDetail) {
    let mode_display = match (&d.mode_requested, &d.mode_resolved) {
        (Some(req), Some(res)) if req == res => req.to_string(),
        (Some(req), Some(res)) => format!("{req} → {res}"),
        _ => "—".to_string(),
    };

    println!(
        "{}   {} · {}    caller={}",
        d.trace_id, d.kind, mode_display, d.caller
    );

    let ts = d.created_at.get(..19).unwrap_or(&d.created_at);
    println!("Time:           {}  ({}ms)", ts, d.latency_ms);

    if let Some(q) = &d.query {
        println!("Query:          {:?}", q);
    }

    if d.kind == "search" {
        let req = d.mode_requested.as_deref().unwrap_or("—");
        let res = d.mode_resolved.as_deref().unwrap_or("—");
        println!("Mode requested: {req}     resolved: {res}");
    }

    if let Some(provider) = &d.provider {
        let model = d.provider_model.as_deref().unwrap_or("—");
        println!("Provider:       {provider}   model: {model}");
    }

    if let Some(params) = &d.params {
        if let Some(limit) = params.get("limit") {
            let type_filter = params
                .get("type_filter")
                .and_then(|v| v.as_str())
                .unwrap_or("—");
            println!("Limit: {}      Type filter: {}", limit, type_filter);
        }
    }

    println!();
    println!("Results ({}):", d.result_count);

    if d.result_ids.is_empty() {
        println!("  (none)");
    } else {
        for (i, id) in d.result_ids.iter().enumerate() {
            let score = d.result_scores.get(i).copied();
            match score {
                Some(s) => println!("  {}. {id}  {:.2}", i + 1, s),
                None => println!("  {}. {id}", i + 1),
            }
        }
    }

    if d.kind == "search" {
        if let Some(resolved) = &d.mode_resolved {
            let method = match resolved.as_str() {
                "hybrid" => "lexical+vector merged via reciprocal rank fusion.",
                "semantic" => "vector similarity only.",
                "lexical" => "FTS5 lexical match only.",
                _ => "",
            };
            if !method.is_empty() {
                println!();
                println!("Score parts: {method}");
            }
        }
    }
}

// === TEXT RENDERING — REPLAY ===

fn print_replay(r: &ReplayResult) {
    println!("Replaying {}…", r.trace_id);
    println!();

    println!("Original ({} results):", r.original.result_ids.len());
    if r.original.result_ids.is_empty() {
        println!("  (none)");
    } else {
        for (i, id) in r.original.result_ids.iter().enumerate() {
            let score_str = r
                .original
                .scores
                .get(i)
                .map(|s| format!("  {:.2}", s))
                .unwrap_or_default();
            println!("  {}. {id}{score_str}", i + 1);
        }
    }

    println!();
    println!("Now ({} results):", r.current.result_ids.len());
    if r.current.result_ids.is_empty() {
        println!("  (none)");
    } else {
        let original_score_map: std::collections::HashMap<&str, f64> = r
            .original
            .result_ids
            .iter()
            .zip(r.original.scores.iter())
            .map(|(id, &s)| (id.as_str(), s))
            .collect();

        for (i, id) in r.current.result_ids.iter().enumerate() {
            let score = r.current.scores.get(i).copied();
            let annotation = match (score, original_score_map.get(id.as_str()).copied()) {
                (Some(curr), Some(orig)) => {
                    let delta = curr - orig;
                    if delta.abs() < f64::EPSILON {
                        "   (unchanged)".to_string()
                    } else if delta > 0.0 {
                        format!("   (score +{:.2})", delta)
                    } else {
                        format!("   (score {:.2})", delta)
                    }
                }
                (Some(_), None) => "   (new)".to_string(),
                _ => String::new(),
            };
            let score_str = score.map(|s| format!("  {:.2}", s)).unwrap_or_default();
            println!("  {}. {id}{score_str}{annotation}", i + 1);
        }
    }

    // Surface dropped results.
    if !r.diff.removed.is_empty() {
        for id in &r.diff.removed {
            println!("  ─ {id}   dropped from results");
        }
    }

    println!();

    // Provider line.
    let provider_line = match (r.provider_match, r.mode_fallback) {
        (true, false) => "Provider: matches original.".to_string(),
        (false, true) => "Provider: mismatch or unavailable — ran lexical fallback.".to_string(),
        (false, false) => "Provider: mismatch with original.".to_string(),
        (true, true) => "Provider: matches original (mode fell back).".to_string(),
    };
    println!("{provider_line}");

    // Corpus drift.
    let original_count = r.original.result_ids.len() as i64;
    let current_count = r.current.result_ids.len() as i64;
    let drift = current_count - original_count;
    if drift == 0 {
        println!("Corpus drift: none detected.");
    } else if drift > 0 {
        println!("Corpus drift: +{} result(s) since original.", drift);
    } else {
        println!("Corpus drift: {} result(s) since original.", drift);
    }

    let replay_ts = &r.replay_trace_id;
    println!("Replay trace: {replay_ts}");
}
