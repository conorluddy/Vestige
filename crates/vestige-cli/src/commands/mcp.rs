use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Disable record_* tools.
    #[arg(long)]
    pub read_only: bool,
}

pub fn run(args: McpArgs) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(vestige_mcp::run(vestige_mcp::McpOptions {
        read_only: args.read_only,
    }))
}
