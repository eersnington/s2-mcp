use std::{error::Error as StdError, io, path::PathBuf};

use s2_sdk::types::{S2Error, ValidationError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("S2 access token is not configured")]
    MissingAccessToken,
    #[error("could not determine the S2 configuration directory")]
    ConfigDirectoryNotFound,
    #[error("failed to read configuration at {path}: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },
    #[error("invalid configuration at {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("environment variable {name} is not valid Unicode")]
    InvalidEnvironment { name: &'static str },
    #[error("failed to create log file at {path}: {source}")]
    CreateLogFile { path: PathBuf, source: io::Error },
    #[error("failed to initialize logging: {source}")]
    InitializeLogging {
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error(
        "Could not start S2 Lite. Start Docker or OrbStack, set DOCKER_HOST if the socket is non-default, provide --endpoint URL, or use --from-env. Details: {source}"
    )]
    StartManagedLite { source: s2_testcontainers::Error },
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("operation is not available under the active server policy")]
    Forbidden,
    #[error("basin `{requested}` is outside the configured basin scope `{allowed}`")]
    BasinScope { requested: String, allowed: String },
    #[error("S2 request failed: {0}")]
    S2(#[from] S2Error),
    #[error("invalid S2 value: {0}")]
    Validation(#[from] ValidationError),
    #[error("failed to serialize data: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Code Mode failed: {0}")]
    CodeMode(String),
    #[error("code exceeds the {maximum} byte source limit")]
    SourceTooLarge { maximum: usize },
    #[error("code execution timed out after {seconds} seconds")]
    ExecutionTimeout { seconds: u64 },
    #[error("execution cancelled")]
    ExecutionCancelled,
    #[error("execution request exceeds the {maximum} byte child-process limit")]
    ExecutorRequestTooLarge { maximum: usize },
    #[error("failed to start the execution process: {0}")]
    StartExecutor(io::Error),
    #[error("failed to communicate with the execution process: {0}")]
    ExecutorIo(io::Error),
    #[error("execution process exited unsuccessfully: {0}")]
    ExecutorFailed(String),
    #[error("MCP server failed: {0}")]
    Mcp(String),
}

pub type Result<T> = std::result::Result<T, Error>;
