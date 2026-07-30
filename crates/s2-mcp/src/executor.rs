use std::{
    env,
    io::{self, Write},
    process::Stdio,
    sync::Arc,
};

use s2_mcp_codemode::{ExecuteInput, ExecuteOutput, InvokeError, Invoker, Limits};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

use crate::{
    catalog::Catalog,
    config::ConnectionConfig,
    error::{Error, Result},
    operations::{Operations, dispatch},
    policy::Policy,
};

const MAX_CHILD_REQUEST_BYTES: usize = 512 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct ChildRequest {
    input: ExecuteInput,
    connection: ConnectionConfig,
    policy: Policy,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ChildResponse {
    Success { output: ExecuteOutput },
    Error { message: String },
}

pub(crate) async fn execute_in_subprocess(
    input: ExecuteInput,
    connection: &ConnectionConfig,
    policy: &Policy,
) -> Result<ExecuteOutput> {
    let limits = Limits::default();
    if input.code.len() > limits.max_source_bytes {
        return Err(Error::SourceTooLarge {
            maximum: limits.max_source_bytes,
        });
    }

    let request = ChildRequest {
        input,
        connection: connection.clone(),
        policy: policy.clone(),
    };
    if serialized_size(&request)? > MAX_CHILD_REQUEST_BYTES {
        return Err(Error::ExecutorRequestTooLarge {
            maximum: MAX_CHILD_REQUEST_BYTES,
        });
    }
    let request = serde_json::to_vec(&request)?;

    let executable = env::current_exe().map_err(Error::StartExecutor)?;
    let mut child = Command::new(executable)
        .arg("__execute")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(Error::StartExecutor)?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        Error::ExecutorIo(io::Error::other(
            "execution process stdin was not available",
        ))
    })?;
    stdin.write_all(&request).await.map_err(Error::ExecutorIo)?;
    stdin.shutdown().await.map_err(Error::ExecutorIo)?;
    drop(stdin);

    let output = tokio::time::timeout(limits.execution_timeout, child.wait_with_output())
        .await
        .map_err(|_| Error::ExecutionTimeout {
            seconds: limits.execution_timeout.as_secs(),
        })?
        .map_err(Error::ExecutorIo)?;
    if !output.status.success() {
        return Err(Error::ExecutorFailed(output.status.to_string()));
    }

    match serde_json::from_slice(&output.stdout)? {
        ChildResponse::Success { output } => Ok(output),
        ChildResponse::Error { message } => Err(Error::CodeMode(message)),
    }
}

pub(crate) async fn run_child() -> Result<()> {
    let response = match execute_child_request().await {
        Ok(output) => ChildResponse::Success { output },
        Err(error) => ChildResponse::Error {
            message: error.to_string(),
        },
    };
    let encoded = serde_json::to_vec(&response)?;
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(&encoded)
        .await
        .map_err(Error::ExecutorIo)?;
    stdout.shutdown().await.map_err(Error::ExecutorIo)
}

async fn execute_child_request() -> Result<ExecuteOutput> {
    let mut encoded = Vec::new();
    tokio::io::stdin()
        .take((MAX_CHILD_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .await
        .map_err(Error::ExecutorIo)?;
    if encoded.len() > MAX_CHILD_REQUEST_BYTES {
        return Err(Error::ExecutorRequestTooLarge {
            maximum: MAX_CHILD_REQUEST_BYTES,
        });
    }
    let ChildRequest {
        input,
        connection,
        policy,
    } = serde_json::from_slice(&encoded)?;
    let handler = Arc::new(Operations::new(connection, policy.clone())?);
    let catalog = Catalog::new(&policy)?;
    let code_mode = catalog.code_mode()?;
    let invoker = Invoker::new(move |operation, arguments| {
        let handler = handler.clone();
        let catalog = catalog.clone();
        let policy = policy.clone();
        async move {
            let arguments = arguments.unwrap_or_else(empty_object);
            dispatch(&handler, &catalog, &policy, &operation, arguments)
                .await
                .map_err(map_invoke_error)
        }
    });
    code_mode
        .execute(&input.code, invoker, Limits::default())
        .await
        .map_err(|error| Error::CodeMode(error.to_string()))
}

fn map_invoke_error(error: Error) -> InvokeError {
    match error {
        Error::InvalidArguments(_) | Error::Forbidden | Error::BasinScope { .. } => {
            InvokeError::public(error.to_string())
        }
        _ => InvokeError::private(),
    }
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn serialized_size<T: Serialize + ?Sized>(value: &T) -> Result<usize> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
