//! `vestige daemon pause --for <dur> | --until <rfc3339>` — suppress scheduled ticks.
//!
//! Thin adapter over the `daemon.pause` IPC method (V0.5.2). Exactly one of `--for` /
//! `--until` is required; `--for` is resolved to an absolute UTC instant (`now + dur`)
//! before sending, so the daemon only ever receives an absolute `until`.

use anyhow::{anyhow, Result};
use clap::Args;

use super::ipc_client;

// === TYPES ===

#[derive(Args, Debug)]
pub struct PauseArgs {
    /// Pause for a relative duration, e.g. `30s`, `15m`, `1h`, `2d`.
    #[arg(long, conflicts_with = "until")]
    pub r#for: Option<String>,
    /// Pause until an absolute RFC-3339 instant, e.g. `2026-06-04T08:00:00Z`.
    #[arg(long, conflicts_with = "for")]
    pub until: Option<String>,
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: PauseArgs) -> Result<()> {
    let until = resolve_until(&args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(pause_async(until, args.json))
}

// === PRIVATE HELPERS ===

/// Resolve the absolute RFC-3339 `until` from exactly one of `--for` / `--until`.
fn resolve_until(args: &PauseArgs) -> Result<String> {
    match (&args.r#for, &args.until) {
        (Some(dur), None) => {
            let secs = parse_duration_secs(dur)?;
            let until = time::OffsetDateTime::now_utc() + time::Duration::seconds(secs);
            until
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| anyhow!("formatting resume time: {e}"))
        }
        (None, Some(until)) => Ok(until.clone()),
        (None, None) => Err(anyhow!(
            "one of --for <dur> or --until <rfc3339> is required"
        )),
        (Some(_), Some(_)) => Err(anyhow!("--for and --until are mutually exclusive")),
    }
}

/// Parse a compact duration like `30s`, `15m`, `1h`, `2d`, or a bare integer (seconds).
fn parse_duration_secs(s: &str) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("empty duration"));
    }
    let (num, unit_secs) = match s.chars().last().unwrap() {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3_600),
        'd' => (&s[..s.len() - 1], 86_400),
        c if c.is_ascii_digit() => (s, 1),
        other => return Err(anyhow!("unknown duration unit `{other}` (use s/m/h/d)")),
    };
    let value: i64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid duration `{s}` — expected e.g. 30s, 15m, 1h, 2d"))?;
    if value <= 0 {
        return Err(anyhow!("duration must be positive"));
    }
    Ok(value * unit_secs)
}

async fn pause_async(until: String, json: bool) -> Result<()> {
    let response = ipc_client::call("daemon.pause", serde_json::json!({ "until": until })).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if let Some(err) = response.get("error") {
        println!(
            "pause failed: {} ({})",
            err["message"].as_str().unwrap_or("unknown"),
            err["code"].as_i64().unwrap_or(-1)
        );
    } else if let Some(result) = response.get("result") {
        let paused_until = result["paused_until"].as_str().unwrap_or(&until);
        println!("daemon paused until {paused_until}");
    }
    Ok(())
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compact_durations() {
        assert_eq!(parse_duration_secs("30s").unwrap(), 30);
        assert_eq!(parse_duration_secs("15m").unwrap(), 900);
        assert_eq!(parse_duration_secs("1h").unwrap(), 3_600);
        assert_eq!(parse_duration_secs("2d").unwrap(), 172_800);
        assert_eq!(parse_duration_secs("45").unwrap(), 45); // bare = seconds
    }

    #[test]
    fn rejects_bad_durations() {
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("1y").is_err());
        assert!(parse_duration_secs("abc").is_err());
        assert!(parse_duration_secs("0h").is_err());
    }

    #[test]
    fn requires_exactly_one_of_for_until() {
        let neither = PauseArgs {
            r#for: None,
            until: None,
            json: false,
        };
        assert!(resolve_until(&neither).is_err());

        let only_until = PauseArgs {
            r#for: None,
            until: Some("2999-01-01T00:00:00Z".into()),
            json: false,
        };
        assert_eq!(resolve_until(&only_until).unwrap(), "2999-01-01T00:00:00Z");

        let only_for = PauseArgs {
            r#for: Some("1h".into()),
            until: None,
            json: false,
        };
        // Resolves to a real RFC-3339 instant in the future.
        let resolved = resolve_until(&only_for).unwrap();
        let parsed =
            time::OffsetDateTime::parse(&resolved, &time::format_description::well_known::Rfc3339)
                .unwrap();
        assert!(parsed > time::OffsetDateTime::now_utc());
    }
}
