#[tokio::main]
async fn main() -> anyhow::Result<()> {
    zeron_mcp::run(zeron_mcp::McpConfig::from_env()).await
}
