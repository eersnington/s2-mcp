mod catalog;
mod config;
mod error;
mod executor;
mod mode;
mod operations;
mod policy;
mod server;
mod tool_mode;

use std::sync::Arc;

use catalog::Catalog;
pub use config::{S2Compression, S2Configuration};
pub use error::{Error, Result};
pub use mode::ServerMode;
use operations::{Operations, SharedOperationHandler};
pub use policy::Policy;
use rmcp::{ServiceExt, transport::stdio};
pub use server::{S2McpServer, ServerOptions};

pub async fn serve(options: ServerOptions, configuration: S2Configuration) -> Result<()> {
    let handler: SharedOperationHandler = Arc::new(Operations::new(
        configuration.clone(),
        options.policy.clone(),
    )?);
    let catalog = Catalog::new(&options.policy)?;
    let server = S2McpServer::new(catalog, configuration, handler, &options)?;
    let service = server
        .serve(stdio())
        .await
        .map_err(|error| Error::Mcp(error.to_string()))?;
    service
        .waiting()
        .await
        .map(|_| ())
        .map_err(|error| Error::Mcp(error.to_string()))
}

#[doc(hidden)]
pub async fn run_executor_child() -> Result<()> {
    executor::run_child().await
}
