//! MCP server adapter — stub for M5. Currently a no-op placeholder so the CLI
//! can wire up a `vestige mcp` subcommand without pulling rmcp into the M0
//! build.

use anyhow::Result;

pub struct McpOptions {
    pub read_only: bool,
}

pub fn run(_opts: McpOptions) -> Result<()> {
    anyhow::bail!("MCP server not implemented yet — landing in M5");
}
