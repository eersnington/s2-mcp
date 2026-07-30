use rmcp::{
    ErrorData as McpError, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use s2_mcp_codemode::{
    CodeMode, ExecuteInput, ExecuteOutput, Limits, SearchInput, SearchOutput, validate_json_depth,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    catalog::{Catalog, json_object_schema},
    config::S2Configuration,
    error::{Error, Result},
    executor::execute_in_subprocess,
    mode::ServerMode,
    operations::SharedOperationHandler,
    policy::Policy,
    tool_mode::ToolMode,
};

#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub mode: ServerMode,
    pub policy: Policy,
}

#[derive(Clone)]
enum Surface {
    Code(CodeMode),
    Tools(ToolMode),
}

#[derive(Clone)]
pub struct S2McpServer {
    connection: S2Configuration,
    policy: Policy,
    surface: Surface,
}

impl S2McpServer {
    pub(crate) fn new(
        catalog: Catalog,
        connection: S2Configuration,
        handler: SharedOperationHandler,
        options: &ServerOptions,
    ) -> Result<Self> {
        let surface = match options.mode {
            ServerMode::Code => Surface::Code(catalog.code_mode()?),
            ServerMode::Tools => {
                Surface::Tools(ToolMode::new(catalog, handler, options.policy.clone()))
            }
        };
        Ok(Self {
            connection,
            policy: options.policy.clone(),
            surface,
        })
    }

    fn tools(&self) -> Result<Vec<Tool>> {
        match &self.surface {
            Surface::Code(_) => {
                let mut tools = vec![self.search_tool()?, self.execute_tool()?];
                tools.sort_by(|left, right| left.name.cmp(&right.name));
                Ok(tools)
            }
            Surface::Tools(tool_mode) => Ok(tool_mode.tools()),
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

    async fn call_code_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        validate_json_depth(&arguments, Limits::default().max_json_depth)
            .map_err(|error| Error::CodeMode(error.to_string()))?;
        let Surface::Code(code_catalog) = &self.surface else {
            return Err(Error::Forbidden);
        };
        match name {
            "search" => {
                let input = decode(arguments)?;
                let output = code_catalog
                    .search(input)
                    .map_err(|error| Error::CodeMode(error.to_string()))?;
                Ok(serde_json::to_value(output)?)
            }
            "execute" => {
                let input = decode(arguments)?;
                Ok(serde_json::to_value(
                    execute_in_subprocess(input, &self.connection, &self.policy).await?,
                )?)
            }
            _ => Err(Error::Forbidden),
        }
    }
}

impl ServerHandler for S2McpServer {
    fn get_info(&self) -> ServerInfo {
        let instructions = match self.surface {
            Surface::Code(_) => {
                "Call search to discover typed S2 functions, then execute one bounded TypeScript program."
            }
            Surface::Tools(_) => {
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
        match &self.surface {
            Surface::Code(_) => self
                .tools()
                .ok()
                .and_then(|tools| tools.into_iter().find(|tool| tool.name == name)),
            Surface::Tools(tool_mode) => tool_mode.get_tool(name),
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
        let result = match &self.surface {
            Surface::Code(_) => {
                tokio::select! {
                    biased;
                    _ = context.ct.cancelled() => Err(Error::ExecutionCancelled),
                    result = self.call_code_tool(&name, arguments) => result,
                }
            }
            Surface::Tools(tool_mode) => {
                tokio::select! {
                    biased;
                    _ = context.ct.cancelled() => Err(Error::ExecutionCancelled),
                    result = tool_mode.dispatch(&name, arguments) => result,
                }
            }
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value).into(),
            Err(error) => CallToolResult::structured_error(serde_json::json!({
                "error": error.to_string(),
            }))
            .into(),
        })
    }
}

fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|error| Error::InvalidArguments(error.to_string()))
}

fn internal_error(error: Error) -> McpError {
    McpError::internal_error(error.to_string(), None)
}
