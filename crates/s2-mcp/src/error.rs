use std::{error::Error as StdError, io, path::PathBuf};

use s2_sdk::types::{S2Error, ValidationError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    MissingAccessToken,
    ConfigDirectoryNotFound,
    ConfigReadFailed,
    ConfigParseFailed,
    InvalidEnvironment,
    LogFileCreationFailed,
    LoggingInitializationFailed,
    InvalidConfiguration,
    ManagedLiteUnavailable,
    InvalidArguments,
    OperationForbidden,
    BasinOutOfScope,
    S2RequestFailed,
    InvalidS2Value,
    CodeModeFailed,
    SourceTooLarge,
    ExecutionTimeout,
    ExecutionCancelled,
    ExecutorRequestTooLarge,
    ExecutorStartFailed,
    ExecutorIoFailed,
    ExecutorFailed,
    SerializationFailed,
    McpServerFailed,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingAccessToken => "missing_access_token",
            Self::ConfigDirectoryNotFound => "config_directory_not_found",
            Self::ConfigReadFailed => "config_read_failed",
            Self::ConfigParseFailed => "config_parse_failed",
            Self::InvalidEnvironment => "invalid_environment",
            Self::LogFileCreationFailed => "log_file_creation_failed",
            Self::LoggingInitializationFailed => "logging_initialization_failed",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::ManagedLiteUnavailable => "managed_lite_unavailable",
            Self::InvalidArguments => "invalid_arguments",
            Self::OperationForbidden => "operation_forbidden",
            Self::BasinOutOfScope => "basin_out_of_scope",
            Self::S2RequestFailed => "s2_request_failed",
            Self::InvalidS2Value => "invalid_s2_value",
            Self::CodeModeFailed => "code_mode_failed",
            Self::SourceTooLarge => "source_too_large",
            Self::ExecutionTimeout => "execution_timeout",
            Self::ExecutionCancelled => "execution_cancelled",
            Self::ExecutorRequestTooLarge => "executor_request_too_large",
            Self::ExecutorStartFailed => "executor_start_failed",
            Self::ExecutorIoFailed => "executor_io_failed",
            Self::ExecutorFailed => "executor_failed",
            Self::SerializationFailed => "serialization_failed",
            Self::McpServerFailed => "mcp_server_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDiagnostic {
    pub code: ErrorCode,
    pub message: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ErrorReport {
    Public(ErrorDiagnostic),
    Private,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("S2 access token is not configured")]
    MissingAccessToken,
    #[error("could not determine the S2 configuration directory")]
    DirectoryNotFound,
    #[error("failed to read configuration at {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("invalid configuration at {path}: {source}")]
    Parse {
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
    Invalid(String),
    #[error(
        "Could not start S2 Lite. Start Docker or OrbStack, set DOCKER_HOST if the socket is non-default, provide --endpoint URL, or use --from-env. Details: {source}"
    )]
    StartManagedLite { source: s2_testcontainers::Error },
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("operation is not available under the active server policy")]
    Forbidden,
    #[error("basin `{requested}` is outside the configured basin scope `{allowed}`")]
    BasinScope { requested: String, allowed: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("S2 request failed: {0}")]
    S2(#[source] S2Error),
    #[error("invalid S2 value: {0}")]
    Validation(#[source] ValidationError),
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Code Mode failed: {message}")]
    CodeMode { message: String, code: ErrorCode },
    #[error("code exceeds the {maximum} byte source limit")]
    SourceTooLarge { maximum: usize },
    #[error("code execution timed out after {seconds} seconds")]
    Timeout { seconds: u64 },
    #[error("execution cancelled")]
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("execution request exceeds the {maximum} byte child-process limit")]
    RequestTooLarge { maximum: usize },
    #[error("failed to start the execution process: {0}")]
    Start(io::Error),
    #[error("failed to communicate with the execution process: {0}")]
    Io(io::Error),
    #[error("execution process exited unsuccessfully: {0}")]
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("failed to serialize data: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("MCP server failed: {0}")]
    Mcp(String),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

impl From<S2Error> for Error {
    fn from(error: S2Error) -> Self {
        Self::Service(ServiceError::S2(error))
    }
}

impl From<ValidationError> for Error {
    fn from(error: ValidationError) -> Self {
        Self::Service(ServiceError::Validation(error))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Protocol(ProtocolError::Serialize(error))
    }
}

impl Error {
    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::Request(RequestError::InvalidArguments(message.into()))
    }

    pub fn invalid_configuration(message: impl Into<String>) -> Self {
        Self::Config(ConfigError::Invalid(message.into()))
    }

    pub fn code_mode(message: impl Into<String>) -> Self {
        Self::Execution(ExecutionError::CodeMode {
            message: message.into(),
            code: ErrorCode::CodeModeFailed,
        })
    }

    pub(crate) fn code_mode_with_diagnostic(diagnostic: ErrorDiagnostic) -> Self {
        Self::Execution(ExecutionError::CodeMode {
            message: diagnostic.message,
            code: diagnostic.code,
        })
    }

    pub(crate) fn report(&self) -> ErrorReport {
        if self.is_tool_failure() {
            ErrorReport::Public(self.diagnostic())
        } else {
            ErrorReport::Private
        }
    }

    pub const fn forbidden() -> Self {
        Self::Policy(PolicyError::Forbidden)
    }

    pub fn basin_scope(requested: impl Into<String>, allowed: impl Into<String>) -> Self {
        Self::Policy(PolicyError::BasinScope {
            requested: requested.into(),
            allowed: allowed.into(),
        })
    }

    pub const fn execution_cancelled() -> Self {
        Self::Execution(ExecutionError::Cancelled)
    }

    pub const fn execution_timeout(seconds: u64) -> Self {
        Self::Execution(ExecutionError::Timeout { seconds })
    }

    pub fn executor_failed(message: impl Into<String>) -> Self {
        Self::Sandbox(SandboxError::Failed(message.into()))
    }

    pub fn executor_io(error: io::Error) -> Self {
        Self::Sandbox(SandboxError::Io(error))
    }

    pub fn start_executor(error: io::Error) -> Self {
        Self::Sandbox(SandboxError::Start(error))
    }

    pub const fn executor_request_too_large(maximum: usize) -> Self {
        Self::Sandbox(SandboxError::RequestTooLarge { maximum })
    }

    pub const fn is_tool_failure(&self) -> bool {
        matches!(
            self,
            Self::Config(_)
                | Self::Request(_)
                | Self::Policy(_)
                | Self::Service(_)
                | Self::Execution(_)
        )
    }

    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Config(error) => match error {
                ConfigError::MissingAccessToken => ErrorCode::MissingAccessToken,
                ConfigError::DirectoryNotFound => ErrorCode::ConfigDirectoryNotFound,
                ConfigError::Read { .. } => ErrorCode::ConfigReadFailed,
                ConfigError::Parse { .. } => ErrorCode::ConfigParseFailed,
                ConfigError::InvalidEnvironment { .. } => ErrorCode::InvalidEnvironment,
                ConfigError::CreateLogFile { .. } => ErrorCode::LogFileCreationFailed,
                ConfigError::InitializeLogging { .. } => ErrorCode::LoggingInitializationFailed,
                ConfigError::Invalid(_) => ErrorCode::InvalidConfiguration,
                ConfigError::StartManagedLite { .. } => ErrorCode::ManagedLiteUnavailable,
            },
            Self::Request(_) => ErrorCode::InvalidArguments,
            Self::Policy(error) => match error {
                PolicyError::Forbidden => ErrorCode::OperationForbidden,
                PolicyError::BasinScope { .. } => ErrorCode::BasinOutOfScope,
            },
            Self::Service(error) => match error {
                ServiceError::S2(_) => ErrorCode::S2RequestFailed,
                ServiceError::Validation(_) => ErrorCode::InvalidS2Value,
            },
            Self::Execution(error) => match error {
                ExecutionError::CodeMode { code, .. } => *code,
                ExecutionError::SourceTooLarge { .. } => ErrorCode::SourceTooLarge,
                ExecutionError::Timeout { .. } => ErrorCode::ExecutionTimeout,
                ExecutionError::Cancelled => ErrorCode::ExecutionCancelled,
            },
            Self::Sandbox(error) => match error {
                SandboxError::RequestTooLarge { .. } => ErrorCode::ExecutorRequestTooLarge,
                SandboxError::Start(_) => ErrorCode::ExecutorStartFailed,
                SandboxError::Io(_) => ErrorCode::ExecutorIoFailed,
                SandboxError::Failed(_) => ErrorCode::ExecutorFailed,
            },
            Self::Protocol(error) => match error {
                ProtocolError::Serialize(_) => ErrorCode::SerializationFailed,
                ProtocolError::Mcp(_) => ErrorCode::McpServerFailed,
            },
        }
    }

    pub fn diagnostic(&self) -> ErrorDiagnostic {
        ErrorDiagnostic {
            code: self.code(),
            message: self.to_string(),
            remediation: self.remediation().map(str::to_owned),
        }
    }

    pub const fn remediation(&self) -> Option<&'static str> {
        match self {
            Self::Config(error) => match error {
                ConfigError::MissingAccessToken => {
                    Some("Set S2_ACCESS_TOKEN or provide an access token in the S2 configuration.")
                }
                ConfigError::DirectoryNotFound => {
                    Some("Set a platform configuration directory before starting the server.")
                }
                ConfigError::Read { .. } => {
                    Some("Check the configuration path and its read permissions.")
                }
                ConfigError::Parse { .. } | ConfigError::Invalid(_) => {
                    Some("Fix the configuration values and retry the server startup.")
                }
                ConfigError::InvalidEnvironment { .. } => {
                    Some("Set the environment variable to valid UTF-8 text.")
                }
                ConfigError::CreateLogFile { .. } => {
                    Some("Choose a writable log-file path or omit the log-file option.")
                }
                ConfigError::InitializeLogging { .. } => {
                    Some("Check the logging configuration and retry the server startup.")
                }
                ConfigError::StartManagedLite { .. } => Some(
                    "Start Docker or OrbStack, set DOCKER_HOST when needed, provide --endpoint, or use --from-env.",
                ),
            },
            Self::Request(_) => {
                Some("Check the tool input schema and provide valid JSON arguments.")
            }
            Self::Policy(error) => match error {
                PolicyError::Forbidden => Some(
                    "Use --allow-destructive for destructive tools and remove --readonly for write operations.",
                ),
                PolicyError::BasinScope { .. } => Some(
                    "Use a basin within the configured --basin scope or restart without --basin.",
                ),
            },
            Self::Service(error) => match error {
                ServiceError::S2(_) => Some(
                    "Check the S2 endpoints, access token, and service availability before retrying.",
                ),
                ServiceError::Validation(_) => {
                    Some("Check the value against the advertised S2 tool schema.")
                }
            },
            Self::Execution(error) => match error {
                ExecutionError::CodeMode { .. } => {
                    Some("Review the Code Mode diagnostic and correct the submitted program.")
                }
                ExecutionError::SourceTooLarge { .. } => {
                    Some("Reduce the TypeScript source or split it across smaller executions.")
                }
                ExecutionError::Timeout { .. } => {
                    Some("Reduce the program's work or split it across smaller executions.")
                }
                ExecutionError::Cancelled => {
                    Some("Retry the tool call; the previous request was cancelled.")
                }
            },
            Self::Sandbox(_) | Self::Protocol(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::{Error, ErrorCode, ErrorDiagnostic, ProtocolError};

    #[test]
    fn policy_errors_have_stable_public_diagnostics() {
        let error = Error::forbidden();
        let diagnostic = error.diagnostic();

        assert_eq!(diagnostic.code, ErrorCode::OperationForbidden);
        assert_eq!(diagnostic.code.as_str(), "operation_forbidden");
        assert!(diagnostic.message.contains("active server policy"));
        assert!(diagnostic.remediation.is_some());
        assert!(error.is_tool_failure());
    }

    #[test]
    fn internal_errors_are_not_public_tool_failures() {
        let error = Error::Protocol(ProtocolError::Mcp("private detail".to_owned()));

        assert_eq!(error.code(), ErrorCode::McpServerFailed);
        assert!(!error.is_tool_failure());
        assert!(error.remediation().is_none());
    }

    #[test]
    fn diagnostics_have_a_stable_json_shape() {
        let diagnostic = ErrorDiagnostic {
            code: ErrorCode::ExecutionTimeout,
            message: "timed out".to_owned(),
            remediation: Some("retry".to_owned()),
        };
        let value = serde_json::to_value(diagnostic);
        assert!(value.is_ok());
        let Ok(value) = value else {
            return;
        };

        assert_eq!(value["code"], "execution_timeout");
        assert_eq!(value["message"], "timed out");
        assert_eq!(value["remediation"], "retry");
    }
}
