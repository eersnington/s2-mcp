use std::{
    env, io,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use s2_mcp_codemode::{CodeMode, ExecuteInput, ExecuteOutput, InvokeError, Invoker, Limits};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, MutexGuard, OnceCell, OwnedSemaphorePermit, Semaphore},
};

use crate::{
    catalog::Catalog,
    config::ConnectionConfig,
    error::{Error, ErrorReport, Result},
    operations::Operations,
    policy::Policy,
};

pub(crate) const MAX_CONCURRENT_EXECUTORS: usize = 4;

const MAX_CHILD_FRAME_BYTES: usize = 512 * 1024;
const EXECUTOR_COMMAND: &str = "__execute";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerRequest {
    Initialize {
        connection: ConnectionConfig,
        policy: Policy,
    },
    Execute {
        input: ExecuteInput,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WorkerResponse {
    Ready,
    Success { output: ExecuteOutput },
    Error { error: ErrorReport },
}

#[derive(Clone)]
pub(crate) struct ExecutorPool {
    workers: Arc<Vec<Arc<Mutex<Worker>>>>,
    permits: Arc<Semaphore>,
    next_worker: Arc<AtomicUsize>,
    executable: Arc<std::path::PathBuf>,
    connection: ConnectionConfig,
    policy: Policy,
}

impl ExecutorPool {
    pub(crate) async fn new(connection: ConnectionConfig, policy: Policy) -> Result<Self> {
        let executable = Arc::new(env::current_exe().map_err(Error::start_executor)?);
        let mut workers = Vec::with_capacity(MAX_CONCURRENT_EXECUTORS);
        for _ in 0..MAX_CONCURRENT_EXECUTORS {
            let worker = Worker::spawn(&executable, &connection, &policy).await?;
            workers.push(Arc::new(Mutex::new(worker)));
        }

        Ok(Self {
            workers: Arc::new(workers),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_EXECUTORS)),
            next_worker: Arc::new(AtomicUsize::new(0)),
            executable,
            connection,
            policy,
        })
    }

    pub(crate) async fn execute(&self, input: ExecuteInput) -> Result<ExecuteOutput> {
        let _permit = self.acquire_permit().await?;
        let mut lease = self.acquire_worker().await;
        let result = lease
            .worker
            .execute(&self.executable, &self.connection, &self.policy, input)
            .await;
        lease.reusable = !lease.worker.needs_restart;
        result
    }

    async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit> {
        self.permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| Error::executor_failed(format!("executor pool closed: {error}")))
    }

    async fn acquire_worker(&self) -> WorkerLease<'_> {
        let start = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        for offset in 0..self.workers.len() {
            let index = (start + offset) % self.workers.len();
            let Ok(worker) = self.workers[index].try_lock() else {
                continue;
            };
            return WorkerLease {
                worker,
                reusable: false,
            };
        }

        WorkerLease {
            worker: self.workers[start].lock().await,
            reusable: false,
        }
    }
}

struct WorkerLease<'pool> {
    worker: MutexGuard<'pool, Worker>,
    reusable: bool,
}

impl Drop for WorkerLease<'_> {
    fn drop(&mut self) {
        if !self.reusable {
            self.worker.abort();
        }
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    needs_restart: bool,
}

impl Worker {
    async fn spawn(
        executable: &Path,
        connection: &ConnectionConfig,
        policy: &Policy,
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
        let Some(stdin) = child.stdin.take() else {
            return Err(Error::executor_io(io::Error::other(
                "execution process stdin was not available",
            )));
        };
        let Some(stdout) = child.stdout.take() else {
            return Err(Error::executor_io(io::Error::other(
                "execution process stdout was not available",
            )));
        };

        let mut worker = Self {
            child,
            stdin,
            stdout,
            needs_restart: false,
        };
        write_frame(
            &mut worker.stdin,
            &WorkerRequest::Initialize {
                connection: connection.clone(),
                policy: policy.clone(),
            },
        )
        .await?;
        let response = read_frame(&mut worker.stdout).await?;
        let Some(WorkerResponse::Ready) = response else {
            return Err(Error::executor_failed(
                "execution worker did not complete initialization",
            ));
        };
        Ok(worker)
    }

    async fn execute(
        &mut self,
        executable: &Path,
        connection: &ConnectionConfig,
        policy: &Policy,
        input: ExecuteInput,
    ) -> Result<ExecuteOutput> {
        self.ensure_ready(executable, connection, policy).await?;

        if let Err(error) = write_frame(&mut self.stdin, &WorkerRequest::Execute { input }).await {
            self.abort();
            return Err(error);
        }

        let response = match tokio::time::timeout(
            Limits::default().execution_timeout,
            read_frame(&mut self.stdout),
        )
        .await
        {
            Ok(Ok(Some(response))) => response,
            Ok(Ok(None)) => {
                self.abort();
                return Err(Error::executor_failed(
                    "execution worker closed before responding",
                ));
            }
            Ok(Err(error)) => {
                self.abort();
                return Err(error);
            }
            Err(_) => {
                self.abort();
                return Err(Error::execution_timeout(
                    Limits::default().execution_timeout.as_secs(),
                ));
            }
        };

        match response {
            WorkerResponse::Success { output } => Ok(output),
            WorkerResponse::Error { error } => Err(match error {
                ErrorReport::Public(diagnostic) => Error::code_mode_with_diagnostic(diagnostic),
                ErrorReport::Private => {
                    Error::executor_failed("execution worker returned an internal error")
                }
            }),
            WorkerResponse::Ready => {
                self.abort();
                Err(Error::executor_failed(
                    "execution worker returned an unexpected initialization response",
                ))
            }
        }
    }

    async fn ensure_ready(
        &mut self,
        executable: &Path,
        connection: &ConnectionConfig,
        policy: &Policy,
    ) -> Result<()> {
        if !self.needs_restart {
            return Ok(());
        }

        self.child.wait().await.map_err(Error::executor_io)?;
        *self = Self::spawn(executable, connection, policy).await?;
        Ok(())
    }

    fn abort(&mut self) {
        self.needs_restart = true;
        if let Err(error) = self.child.start_kill()
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(error = %error, "failed to terminate execution worker");
        }
    }
}

pub(crate) async fn run_child() -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let Some(request) = read_frame(&mut stdin).await? else {
        return Err(Error::executor_failed(
            "execution worker received no initialization request",
        ));
    };
    let WorkerRequest::Initialize { connection, policy } = request else {
        return Err(Error::executor_failed(
            "execution worker was not initialized before use",
        ));
    };
    let state = ChildState::new(connection, policy)?;
    write_frame(&mut stdout, &WorkerResponse::Ready).await?;

    loop {
        let Some(request) = read_frame(&mut stdin).await? else {
            return Ok(());
        };
        match request {
            WorkerRequest::Initialize { .. } => {
                return Err(Error::executor_failed(
                    "execution worker received a duplicate initialization request",
                ));
            }
            WorkerRequest::Execute { input } => {
                let response = match execute_child_request(&state, input).await {
                    Ok(output) => WorkerResponse::Success { output },
                    Err(error) => WorkerResponse::Error {
                        error: error.report(),
                    },
                };
                write_frame(&mut stdout, &response).await?;
            }
        }
    }
}

#[derive(Clone)]
struct ChildState {
    catalog: Catalog,
    code_mode: CodeMode,
    connection: ConnectionConfig,
    operations: Arc<OnceCell<Arc<Operations>>>,
    policy: Arc<Policy>,
}

impl ChildState {
    fn new(connection: ConnectionConfig, policy: Policy) -> Result<Self> {
        let catalog = Catalog::new(&policy)?;
        let code_mode = catalog.code_mode();
        Ok(Self {
            catalog,
            code_mode,
            connection,
            operations: Arc::new(OnceCell::new()),
            policy: Arc::new(policy),
        })
    }
}

async fn execute_child_request(state: &ChildState, input: ExecuteInput) -> Result<ExecuteOutput> {
    let catalog = state.catalog.clone();
    let operations = state.operations.clone();
    let connection = state.connection.clone();
    let policy = state.policy.clone();
    let invoker = Invoker::new(move |operation, arguments| {
        let catalog = catalog.clone();
        let operations = operations.clone();
        let connection = connection.clone();
        let policy = policy.clone();
        async move {
            let operation_id = catalog
                .find(&operation)
                .ok_or_else(Error::forbidden)
                .map_err(map_invoke_error)?
                .id;
            let operations = operations
                .get_or_try_init(|| async {
                    Operations::new(connection.clone(), policy.clone()).map(Arc::new)
                })
                .await
                .map_err(map_invoke_error)?;
            let arguments = arguments.unwrap_or_else(empty_object);
            operations
                .dispatch(operation_id, arguments)
                .await
                .map_err(map_invoke_error)
        }
    });
    state
        .code_mode
        .execute(&input.code, invoker, Limits::default())
        .await
        .map_err(|error| Error::code_mode(error.to_string()))
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

fn empty_object() -> Value {
    Value::Object(Default::default())
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
