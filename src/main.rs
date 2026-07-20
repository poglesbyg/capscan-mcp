use anyhow::Result;
use capscan_mcp::CapscanTools;
use rmcp::transport::stdio;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<()> {
    let service = CapscanTools::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
