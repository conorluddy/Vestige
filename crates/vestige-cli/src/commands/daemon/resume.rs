//! `vestige daemon resume` — clear any active pause so scheduled ticks resume.
//!
//! Thin adapter over the `daemon.resume` IPC method (V0.5.2). Idempotent: resuming when not
//! paused is a no-op success.

use anyhow::Result;
use clap::Args;

use super::ipc_client;

// === TYPES ===

#[derive(Args, Debug)]
pub struct ResumeArgs {
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
}

// === PUBLIC API ===

pub fn run(args: ResumeArgs) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(resume_async(args.json))
}

// === PRIVATE HELPERS ===

async fn resume_async(json: bool) -> Result<()> {
    let response = ipc_client::call("daemon.resume", serde_json::json!({})).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if let Some(err) = response.get("error") {
        println!(
            "resume failed: {} ({})",
            err["message"].as_str().unwrap_or("unknown"),
            err["code"].as_i64().unwrap_or(-1)
        );
    } else {
        println!("daemon resumed");
    }
    Ok(())
}
