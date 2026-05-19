//! `vestige daemon status` — read ~/.vestige/daemon.status.json, format.

use clap::Args;
use vestige_daemon::ipc::status_file;

// === TYPES ===

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Output JSON for scripts.
    #[arg(long)]
    pub json: bool,
    /// Refresh every 5 s like `watch`. Press Ctrl-C to exit.
    #[arg(long)]
    pub watch: bool,
}

// === PUBLIC API ===

pub fn run(args: StatusArgs) -> anyhow::Result<()> {
    let path = status_file::resolve_status_path(None);
    if args.watch {
        loop {
            print_status(&path, args.json)?;
            std::thread::sleep(std::time::Duration::from_secs(5));
            // Clear screen only in text mode — JSON output should stream cleanly.
            if !args.json {
                print!("\x1B[2J\x1B[1;1H"); // clear + home (VT100)
            }
        }
    }
    print_status(&path, args.json)
}

// === PRIVATE HELPERS ===

fn print_status(path: &std::path::Path, json: bool) -> anyhow::Result<()> {
    let status = status_file::read(path)
        .map_err(|e| anyhow::anyhow!("failed to read daemon status file: {e}"))?;

    match status {
        None => {
            if json {
                println!("{}", serde_json::json!({ "running": false }));
            } else {
                println!("daemon: not running");
            }
        }
        Some(s) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                println!(
                    "daemon: running  pid={}  uptime={}s  version={}",
                    s.pid, s.uptime_secs, s.version
                );
                println!("projects: {}", s.projects.len());
                for p in &s.projects {
                    println!(
                        "  {} ({}) — pending_embeds={}  last_embed={}",
                        p.project_id,
                        p.project_name,
                        p.pending_embeds,
                        p.last_embed_run.as_deref().unwrap_or("never")
                    );
                }
                if !s.next_jobs.is_empty() {
                    println!("next jobs:");
                    for j in &s.next_jobs {
                        let pid = j.project_id.as_ref().map(|p| p.as_str()).unwrap_or("*");
                        println!("  {:?}  {}  at {}", j.kind, pid, j.at);
                    }
                }
            }
        }
    }

    Ok(())
}
