use std::{collections::BTreeMap, env, io, path::Path, process::Stdio, sync::Arc};

use s2_mcp_codemode::{ExecuteInput, ExecuteOutput, ExecutionApi, InvokeError, Invoker, Limits};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdout, Command},
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore},
};

use crate::{
    catalog::OperationId,
    config::ConnectionConfig,
    error::{Error, ErrorCode, ErrorReport, Result},
    operations::Operations,
    policy::Policy,
};

pub(crate) const MAX_CONCURRENT_EXECUTORS: usize = 4;

const MAX_CHILD_FRAME_BYTES: usize = 512 * 1024;
const EXECUTOR_COMMAND: &str = "__execute";

#[derive(Debug, Serialize, Deserialize)]
struct WorkerRequest {
    connection: ConnectionConfig,
    policy: Policy,
    limits: Limits,
    input: ExecuteInput,
    execution_api: ExecutionApi,
    operation_ids: BTreeMap<String, OperationId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WorkerResponse {
    Success { output: ExecuteOutput },
    Error { error: ErrorReport },
}

#[derive(Clone)]
pub(crate) struct ExecutorPool {
    permits: Arc<Semaphore>,
    executable: Arc<std::path::PathBuf>,
    connection: ConnectionConfig,
    policy: Policy,
    limits: Limits,
    execution_api: ExecutionApi,
    operation_ids: BTreeMap<String, OperationId>,
}

impl ExecutorPool {
    pub(crate) async fn new(
        connection: ConnectionConfig,
        policy: Policy,
        limits: Limits,
        execution_api: ExecutionApi,
        operation_ids: BTreeMap<String, OperationId>,
    ) -> Result<Self> {
        let executable = Arc::new(env::current_exe().map_err(Error::start_executor)?);
        Ok(Self {
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_EXECUTORS)),
            executable,
            connection,
            policy,
            limits,
            execution_api,
            operation_ids,
        })
    }

    pub(crate) async fn execute(&self, input: ExecuteInput) -> Result<ExecuteOutput> {
        let _permit = self.acquire_permit().await?;
        let execution = async {
            let mut worker = Worker::spawn(
                &self.executable,
                self.connection.clone(),
                self.policy.clone(),
                self.limits,
                input,
                self.execution_api.clone(),
                self.operation_ids.clone(),
            )
            .await?;
            worker.response(self.limits).await
        };
        tokio::time::timeout(self.limits.supervisor_timeout(), execution)
            .await
            .map_err(|_| Error::execution_timeout(self.limits.execution_timeout.as_secs()))?
    }

    async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit> {
        self.permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| Error::executor_failed(format!("executor pool closed: {error}")))
    }
}

struct Worker {
    _child: Child,
    stdout: ChildStdout,
}

impl Worker {
    async fn spawn(
        executable: &Path,
        connection: ConnectionConfig,
        policy: Policy,
        limits: Limits,
        input: ExecuteInput,
        execution_api: ExecutionApi,
        operation_ids: BTreeMap<String, OperationId>,
    ) -> Result<Self> {
        let mut child = Command::new(executable)
            .arg(EXECUTOR_COMMAND)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::start_executor)?;
        let Some(mut stdin) = child.stdin.take() else {
            return Err(Error::executor_io(io::Error::other(
                "execution process stdin was not available",
            )));
        };
        let Some(stdout) = child.stdout.take() else {
            return Err(Error::executor_io(io::Error::other(
                "execution process stdout was not available",
            )));
        };

        write_frame(
            &mut stdin,
            &WorkerRequest {
                connection,
                policy,
                limits,
                input,
                execution_api,
                operation_ids,
            },
        )
        .await?;
        Ok(Self {
            _child: child,
            stdout,
        })
    }

    async fn response(&mut self, limits: Limits) -> Result<ExecuteOutput> {
        let Some(response) = read_frame(&mut self.stdout).await? else {
            return Err(Error::executor_failed(
                "execution worker closed before responding",
            ));
        };

        match response {
            WorkerResponse::Success { output } => Ok(output),
            WorkerResponse::Error { error } => Err(match error {
                ErrorReport::Public(diagnostic)
                    if diagnostic.code == ErrorCode::ExecutionTimeout =>
                {
                    Error::execution_timeout(limits.execution_timeout.as_secs())
                }
                ErrorReport::Public(diagnostic) => Error::code_mode_with_diagnostic(diagnostic),
                ErrorReport::Private => {
                    Error::executor_failed("execution worker returned an internal error")
                }
            }),
        }
    }
}

pub(crate) async fn run_child() -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let Some(WorkerRequest {
        connection,
        policy,
        limits,
        input,
        execution_api,
        operation_ids,
    }) = read_frame(&mut stdin).await?
    else {
        return Err(Error::executor_failed(
            "execution worker received no request",
        ));
    };
    let state = ChildState::new(connection, policy, limits, execution_api, operation_ids);
    let response = match execute_child_request(&state, input).await {
        Ok(output) => WorkerResponse::Success { output },
        Err(error) => WorkerResponse::Error {
            error: error.report(),
        },
    };
    write_frame(&mut stdout, &response).await
}

#[derive(Clone)]
struct ChildState {
    execution_api: ExecutionApi,
    operation_ids: BTreeMap<String, OperationId>,
    connection: ConnectionConfig,
    operations: Arc<OnceCell<Arc<Operations>>>,
    policy: Arc<Policy>,
    limits: Limits,
}

impl ChildState {
    fn new(
        connection: ConnectionConfig,
        policy: Policy,
        limits: Limits,
        execution_api: ExecutionApi,
        operation_ids: BTreeMap<String, OperationId>,
    ) -> Self {
        Self {
            execution_api,
            operation_ids,
            connection,
            operations: Arc::new(OnceCell::new()),
            policy: Arc::new(policy),
            limits,
        }
    }
}

async fn execute_child_request(state: &ChildState, input: ExecuteInput) -> Result<ExecuteOutput> {
    let operation_ids = state.operation_ids.clone();
    let operations = state.operations.clone();
    let connection = state.connection.clone();
    let policy = state.policy.clone();
    let invoker = Invoker::new(move |operation, arguments| {
        let operation_ids = operation_ids.clone();
        let operations = operations.clone();
        let connection = connection.clone();
        let policy = policy.clone();
        async move {
            let operation_id = operation_ids
                .get(&operation)
                .copied()
                .ok_or_else(Error::forbidden)
                .map_err(map_invoke_error)?;
            let operations = operations
                .get_or_try_init(|| async {
                    Operations::new(connection.clone(), policy.clone()).map(Arc::new)
                })
                .await
                .map_err(map_invoke_error)?;
            let arguments = arguments.unwrap_or_else(|| Value::Object(Default::default()));
            operations
                .dispatch(operation_id, arguments)
                .await
                .map_err(map_invoke_error)
        }
    });
    state
        .execution_api
        .execute(&input.code, invoker, state.limits)
        .await
        .map_err(Into::into)
}

fn map_invoke_error(error: Error) -> InvokeError {
    if error.is_tool_failure() {
        InvokeError::public_with_details(
            error.code().as_str(),
            error.to_string(),
            error.remediation().map(str::to_owned),
        )
    } else {
        InvokeError::private()
    }
}

async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)?;
    if payload.len() > MAX_CHILD_FRAME_BYTES {
        return Err(Error::executor_request_too_large(MAX_CHILD_FRAME_BYTES));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| Error::executor_request_too_large(MAX_CHILD_FRAME_BYTES))?;
    writer
        .write_all(&length.to_be_bytes())
        .await
        .map_err(Error::executor_io)?;
    writer
        .write_all(&payload)
        .await
        .map_err(Error::executor_io)?;
    writer.flush().await.map_err(Error::executor_io)
}

async fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    let mut prefix_bytes = 0;
    while prefix_bytes < prefix.len() {
        let bytes_read = reader
            .read(&mut prefix[prefix_bytes..])
            .await
            .map_err(Error::executor_io)?;
        if bytes_read == 0 {
            if prefix_bytes == 0 {
                return Ok(None);
            }
            return Err(Error::executor_io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "execution worker closed during a frame header",
            )));
        }
        prefix_bytes += bytes_read;
    }

    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_CHILD_FRAME_BYTES {
        return Err(Error::executor_request_too_large(MAX_CHILD_FRAME_BYTES));
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(Error::executor_io)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(Error::from)
}
