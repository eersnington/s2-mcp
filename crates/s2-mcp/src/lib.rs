use std::sync::Arc;

mod catalog;
mod config;
mod error;
mod executor;
mod launch;
mod mode;
mod operations;
mod policy;
mod server;

pub use config::{S2Compression, S2Configuration};
pub use error::{Error, Result};
pub use launch::{DevSource, LaunchIntent, ResolvedRuntime};
pub use mode::ServerMode;
pub use policy::Policy;
use rmcp::{ServiceExt, transport::stdio};
pub use server::{S2McpServer, ServerOptions};

pub async fn serve(options: ServerOptions, configuration: S2Configuration) -> Result<()> {
    serve_runtime(options, ResolvedRuntime::from_configuration(configuration)).await
}

pub async fn serve_runtime(options: ServerOptions, runtime: ResolvedRuntime) -> Result<()> {
    let server = S2McpServer::new(Arc::new(runtime), &options)?;
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
