use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use s2_mcp_codemode::{
    ExecuteInput, ExecuteOutput, Limits, SearchInput, SearchOutput, validate_json_depth,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{OnceCell, Semaphore};

use crate::{
    catalog::{Catalog, OperationId, json_object_schema},
    config::S2Configuration,
    error::{Error, Result},
    executor::execute_in_subprocess,
    launch::ResolvedRuntime,
    mode::ServerMode,
    operations::Operations,
    policy::Policy,
};

#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub mode: ServerMode,
    pub policy: Policy,
}

#[derive(Clone)]
pub struct S2McpServer {
    runtime: Arc<ResolvedRuntime>,
    catalog: Catalog,
    policy: Policy,
    operations: Arc<OnceCell<Arc<Operations>>>,
    executor_permits: Arc<Semaphore>,
    surface_mode: ServerMode,
}

const MAX_CONCURRENT_EXECUTORS: usize = 4;

impl S2McpServer {
    pub(crate) fn new(runtime: Arc<ResolvedRuntime>, options: &ServerOptions) -> Result<Self> {
        let policy = options.policy.clone();
        let catalog = Catalog::new(&policy)?;
        Ok(Self {
            runtime,
            catalog,
            policy,
            operations: Arc::new(OnceCell::new()),
            executor_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_EXECUTORS)),
            surface_mode: options.mode,
        })
    }

    async fn resolve_operations(&self) -> Result<Arc<Operations>> {
        self.operations
            .get_or_try_init(|| async {
                let connection = self.runtime.configuration().await?;
                Ok(Arc::new(Operations::new(
                    connection.clone(),
                    Arc::new(self.policy.clone()),
                )?))
            })
            .await
            .map(Arc::clone)
    }

    fn tools(&self) -> Result<Vec<Tool>> {
        match self.surface_mode {
            ServerMode::Code => {
                let mut tools = vec![self.search_tool()?, self.execute_tool()?];
                tools.sort_by(|left, right| left.name.cmp(&right.name));
                Ok(tools)
            }
            ServerMode::Tools => {
                let mut tools = self.catalog.tools();
                tools.sort_by(|left, right| left.name.cmp(&right.name));
                Ok(tools)
            }
        }
    }

    fn search_tool(&self) -> Result<Tool> {
        Ok(Tool::new(
            "search",
            "Search the policy-filtered S2 TypeScript API. Returns complete declarations for the matched functions.",
            json_object_schema::<SearchInput>()?,
        )
        .with_raw_output_schema(json_object_schema::<SearchOutput>()?)
        .with_annotations(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        ))
    }

    fn execute_tool(&self) -> Result<Tool> {
        Ok(Tool::new(
            "execute",
            "Execute isolated TypeScript defining `async function run()`. Use `search` first to discover S2 functions.",
            json_object_schema::<ExecuteInput>()?,
        )
        .with_raw_output_schema(json_object_schema::<ExecuteOutput>()?)
        .with_annotations(
            ToolAnnotations::new()
                .read_only(self.policy.readonly)
                .destructive(self.policy.allows_destructive())
                .idempotent(self.policy.readonly)
                .open_world(true),
        ))
    }

    fn call_search(&self, arguments: Value) -> Result<Value> {
        validate_json_depth(&arguments, Limits::default().max_json_depth)
            .map_err(|error| Error::CodeMode(error.to_string()))?;
        let input = decode(arguments)?;
        Ok(serde_json::to_value(self.catalog.search(input)?)?)
    }

    fn prepare_execute(&self, arguments: Value) -> Result<ExecuteInput> {
        validate_json_depth(&arguments, Limits::default().max_json_depth)
            .map_err(|error| Error::CodeMode(error.to_string()))?;
        decode(arguments)
    }

    fn prepare_tool(&self, name: &str, arguments: Value) -> Result<(OperationId, Value)> {
        validate_json_depth(&arguments, Limits::default().max_json_depth)
            .map_err(|error| Error::CodeMode(error.to_string()))?;
        let operation_id = self.catalog.find(name).ok_or(Error::Forbidden)?.id;
        Ok((operation_id, arguments))
    }

    async fn execute_code(
        &self,
        input: ExecuteInput,
        connection: &S2Configuration,
    ) -> Result<Value> {
        let _permit = self
            .executor_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| {
                Error::CodeMode(format!("executor concurrency limiter closed: {error}"))
            })?;
        Ok(serde_json::to_value(
            execute_in_subprocess(input, connection, &self.policy).await?,
        )?)
    }
}

impl ServerHandler for S2McpServer {
    fn get_info(&self) -> ServerInfo {
        let instructions = match self.surface_mode {
            ServerMode::Code => {
                "Call search to discover typed S2 functions, then execute one bounded TypeScript program."
            }
            ServerMode::Tools => {
                "Use the advertised S2 tools directly. Reads and waits are bounded."
            }
        };
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("s2-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("S2 MCP Server")
                    .with_description("Local MCP access to S2 durable streams"),
            )
            .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        self.tools()
            .map(ListToolsResult::with_all_items)
            .map_err(internal_error)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        match self.surface_mode {
            ServerMode::Code => match name {
                "search" => self.search_tool().ok(),
                "execute" => self.execute_tool().ok(),
                _ => None,
            },
            ServerMode::Tools => self
                .catalog
                .find(name)
                .map(|operation| operation.tool.clone()),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, McpError> {
        let name = request.name.into_owned();
        if self.get_tool(&name).is_none() {
            return Err(McpError::invalid_params("tool not found", None));
        }
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = self.dispatch_tool(&name, arguments, &context).await;
        Ok(match result {
            Ok(value) => CallToolResult::structured(value).into(),
            Err(error) => CallToolResult::structured_error(serde_json::json!({
                "error": error.to_string(),
            }))
            .into(),
        })
    }
}

impl S2McpServer {
    async fn dispatch_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &RequestContext<RoleServer>,
    ) -> Result<Value> {
        match (self.surface_mode, name) {
            (ServerMode::Code, "search") => self.call_search(arguments),
            (ServerMode::Code, "execute") => {
                let input = self.prepare_execute(arguments)?;
                let connection = self.runtime.configuration().await?.clone();
                tokio::select! {
                    biased;
                    _ = context.ct.cancelled() => Err(Error::ExecutionCancelled),
                    result = self.execute_code(input, &connection) => result,
                }
            }
            (ServerMode::Tools, _) => {
                let (operation_id, arguments) = self.prepare_tool(name, arguments)?;
                let operations = self.resolve_operations().await?;
                tokio::select! {
                    biased;
                    _ = context.ct.cancelled() => Err(Error::ExecutionCancelled),
                    result = operations.dispatch(operation_id, arguments) => result,
                }
            }
            _ => Err(Error::Forbidden),
        }
    }
}

fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|error| Error::InvalidArguments(error.to_string()))
}

fn internal_error(error: Error) -> McpError {
    McpError::internal_error(error.to_string(), None)
}
