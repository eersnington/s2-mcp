mod catalog;
mod config;
mod error;
mod executor;
mod mode;
mod operation_surface;
mod operations;
mod policy;
mod server;
mod tool_mode;

pub use config::{S2Compression, S2Configuration};
pub use error::{Error, Result};
pub use mode::ServerMode;
use operation_surface::OperationSurface;
pub use policy::Policy;
use rmcp::{ServiceExt, transport::stdio};
pub use server::{S2McpServer, ServerOptions};

pub async fn serve(options: ServerOptions, configuration: S2Configuration) -> Result<()> {
    let surface = OperationSurface::new(configuration.clone(), options.policy.clone())?;
    let server = S2McpServer::new(surface, configuration, &options)?;
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
