//! `vestige review`: list hygiene candidates or forget explicitly selected IDs.

use std::str::FromStr;

use anyhow::{bail, Result};
use clap::Args;
use serde::Serialize;
use time::OffsetDateTime;
use vestige_core::{project_card, MemoryId};
use vestige_engine::review::{forget_review_memory, ReviewForgetStatus};

use crate::context;
use crate::output::emit_json;

#[derive(Debug, Args)]
pub struct ReviewArgs {
    /// Only flag memories older than this many days.
    #[arg(long, default_value_t = 30)]
    pub min_age_days: u32,
    /// Only flag memories strictly below this importance (0..=1).
    #[arg(long, default_value = "0.5", value_parser = parse_importance)]
    pub importance_ceiling: f64,
    /// Soft-delete these explicitly selected IDs, separated by commas.
    #[arg(long, value_delimiter = ',', num_args = 1.., value_name = "ID")]
    pub forget: Vec<String>,
    /// Emit a JSON array.
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
struct ReviewCard {
    id: MemoryId,
    #[serde(rename = "type")]
    memory_type: vestige_core::MemoryType,
    importance: f64,
    recall_count: i64,
    expand_count: i64,
    age_days: i64,
    one_liner: String,
}

#[derive(Serialize)]
struct ForgetOutcome {
    id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

pub fn run(args: ReviewArgs) -> Result<()> {
    let mut ctx = context::load()?;
    if !args.forget.is_empty() {
        let mut failed = false;
        let outcomes: Vec<_> = args
            .forget
            .into_iter()
            .map(|id| {
                let result = MemoryId::from_str(&id)
                    .map_err(anyhow::Error::from)
                    .and_then(|parsed| {
                        forget_review_memory(&mut ctx.store, &ctx.project_id, &parsed)
                            .map_err(anyhow::Error::from)
                    });
                let (status, message) = match result {
                    Ok(status) => {
                        failed |= matches!(
                            status,
                            ReviewForgetStatus::NotFound | ReviewForgetStatus::OutOfScope
                        );
                        (status.as_str().to_owned(), None)
                    }
                    Err(error) => {
                        failed = true;
                        ("error".to_owned(), Some(error.to_string()))
                    }
                };
                ForgetOutcome {
                    id,
                    status,
                    message,
                }
            })
            .collect();
        if args.json {
            emit_json(&outcomes)?;
        } else {
            for outcome in outcomes {
                println!("{}  {}", outcome.id, outcome.status);
                if let Some(message) = outcome.message {
                    eprintln!("  {message}");
                }
            }
        }
        if failed {
            bail!("one or more selected memories could not be forgotten; see per-ID outcomes");
        }
        return Ok(());
    }

    let now = OffsetDateTime::now_utc();
    let cards: Vec<_> = ctx
        .store
        .list_review_candidates(&ctx.project_id, args.min_age_days, args.importance_ceiling)?
        .iter()
        .map(|fetched| ReviewCard {
            id: fetched.memory.id.clone(),
            memory_type: fetched.memory.r#type,
            importance: fetched.memory.importance,
            recall_count: fetched.memory.recall_count,
            expand_count: fetched.memory.expand_count,
            age_days: (now - fetched.memory.created_at).whole_days(),
            one_liner: project_card(fetched).one_liner,
        })
        .collect();
    if args.json {
        emit_json(&cards)
    } else {
        if cards.is_empty() {
            println!("(no memories need review)");
        }
        for card in cards {
            println!(
                "{}  {}  {}d  {}",
                card.id, card.memory_type, card.age_days, card.one_liner
            );
        }
        Ok(())
    }
}

fn parse_importance(value: &str) -> std::result::Result<f64, String> {
    let importance: f64 = value.parse().map_err(|_| "expected a number from 0 to 1")?;
    if !importance.is_finite() || !(0.0..=1.0).contains(&importance) {
        return Err("importance ceiling must be a finite number from 0 to 1".into());
    }
    Ok(importance)
}
