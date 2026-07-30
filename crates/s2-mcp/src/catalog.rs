use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use s2_mcp_codemode::{CodeMode, FunctionDescriptor};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{Error, Result},
    operations::{
        account::{
            ConnectionInfoInput, ConnectionInfoOutput, DeleteBasinRequest, EnsureBasinOutput,
            EnsureBasinRequest, GetBasinConfigOutput, GetBasinConfigRequest, GetMetricsRequest,
            ListBasinsOutput, ListBasinsRequest, MetricsOutput, ReconfigureBasinRequest,
            RevokeAccessTokenOutput, RevokeAccessTokenRequest,
        },
        basin::{
            DeleteResourceOutput, DeleteStreamRequest, DiffResourcesOutput, DiffResourcesRequest,
            EnsureStreamOutput, EnsureStreamRequest, GetStreamConfigOutput, GetStreamConfigRequest,
            ListStreamsOutput, ListStreamsRequest, ReconfigureStreamRequest,
        },
        records::{
            AppendRecordsOutput, AppendRecordsRequest, ReadRecordsOutput, ReadRecordsRequest,
            WaitForRecordsOutput, WaitForRecordsRequest,
        },
        stream::{
            FenceStreamRequest, PositionOutput, StreamCommandOutput, StreamRequest,
            TrimStreamRequest,
        },
    },
    policy::{Access, Policy, Scope},
};

#[derive(Debug, Clone)]
pub(crate) struct Operation {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) access: Access,
    pub(crate) scope: Scope,
    pub(crate) tool: Tool,
}

#[derive(Debug, Clone)]
pub(crate) struct Catalog {
    operations: Arc<[Operation]>,
}

impl Catalog {
    pub(crate) fn new(policy: &Policy) -> Result<Self> {
        let candidates = [
            operation::<ConnectionInfoInput, ConnectionInfoOutput>(
                "connection_info",
                "Describe the active S2 endpoint and server policy without exposing credentials.",
                Access::Read,
                Scope::Global,
                true,
            )?,
            operation::<ListBasinsRequest, ListBasinsOutput>(
                "list_basins",
                "List a bounded page of basins in the S2 account.",
                Access::Read,
                Scope::Account,
                true,
            )?,
            operation::<GetBasinConfigRequest, GetBasinConfigOutput>(
                "get_basin_config",
                "Get the effective configuration of an S2 basin.",
                Access::Read,
                Scope::Basin,
                true,
            )?,
            operation::<ListStreamsRequest, ListStreamsOutput>(
                "list_streams",
                "List a bounded page of streams in an S2 basin.",
                Access::Read,
                Scope::Basin,
                true,
            )?,
            operation::<GetStreamConfigRequest, GetStreamConfigOutput>(
                "get_stream_config",
                "Get the effective configuration of an S2 stream.",
                Access::Read,
                Scope::Stream,
                true,
            )?,
            operation::<StreamRequest, PositionOutput>(
                "check_tail",
                "Get the current tail sequence number and timestamp for an S2 stream.",
                Access::Read,
                Scope::Stream,
                true,
            )?,
            operation::<ReadRecordsRequest, ReadRecordsOutput>(
                "read_records",
                "Read a bounded batch of records from an S2 stream without waiting.",
                Access::Read,
                Scope::Stream,
                true,
            )?,
            operation::<WaitForRecordsRequest, WaitForRecordsOutput>(
                "wait_for_records",
                "Wait for and read a bounded batch of records from an S2 stream.",
                Access::Read,
                Scope::Stream,
                true,
            )?,
            operation::<DiffResourcesRequest, DiffResourcesOutput>(
                "diff_resources",
                "Compare desired basin or stream configurations with their current S2 state.",
                Access::Read,
                Scope::Dynamic {
                    applicable_under_basin: true,
                },
                true,
            )?,
            operation::<GetMetricsRequest, MetricsOutput>(
                "get_metrics",
                "Get bounded account, basin, or stream metrics from S2.",
                Access::Read,
                Scope::Dynamic {
                    applicable_under_basin: true,
                },
                true,
            )?,
            operation::<EnsureBasinRequest, EnsureBasinOutput>(
                "ensure_basin",
                "Create an S2 basin or update it to the requested configuration.",
                Access::Write,
                Scope::Basin,
                true,
            )?,
            operation::<EnsureStreamRequest, EnsureStreamOutput>(
                "ensure_stream",
                "Create an S2 stream or update it to the requested configuration.",
                Access::Write,
                Scope::Stream,
                true,
            )?,
            operation::<AppendRecordsRequest, AppendRecordsOutput>(
                "append_records",
                "Atomically append a bounded batch of records to an S2 stream.",
                Access::Write,
                Scope::Stream,
                false,
            )?,
            operation::<ReconfigureBasinRequest, GetBasinConfigOutput>(
                "reconfigure_basin",
                "Apply a partial configuration update to an S2 basin.",
                Access::Write,
                Scope::Basin,
                true,
            )?,
            operation::<ReconfigureStreamRequest, GetStreamConfigOutput>(
                "reconfigure_stream",
                "Apply a partial configuration update to an S2 stream.",
                Access::Write,
                Scope::Stream,
                true,
            )?,
            operation::<FenceStreamRequest, StreamCommandOutput>(
                "fence_stream",
                "Set or clear the fencing token on an S2 stream.",
                Access::Write,
                Scope::Stream,
                false,
            )?,
            operation::<DeleteBasinRequest, DeleteResourceOutput>(
                "delete_basin",
                "Delete an S2 basin.",
                Access::Destructive,
                Scope::Basin,
                true,
            )?,
            operation::<DeleteStreamRequest, DeleteResourceOutput>(
                "delete_stream",
                "Delete an S2 stream.",
                Access::Destructive,
                Scope::Stream,
                true,
            )?,
            operation::<TrimStreamRequest, StreamCommandOutput>(
                "trim_stream",
                "Advance the earliest retained sequence number of an S2 stream.",
                Access::Destructive,
                Scope::Stream,
                false,
            )?,
            operation::<RevokeAccessTokenRequest, RevokeAccessTokenOutput>(
                "revoke_access_token",
                "Revoke an S2 access token by ID.",
                Access::Destructive,
                Scope::Account,
                true,
            )?,
        ];
        let operations = candidates
            .into_iter()
            .filter(|operation| policy.allows(operation.access, operation.scope))
            .collect::<Vec<_>>()
            .into();
        Ok(Self { operations })
    }

    pub(crate) fn find(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|operation| operation.name == name)
    }

    pub(crate) fn tools(&self) -> Vec<Tool> {
        self.operations
            .iter()
            .map(|operation| operation.tool.clone())
            .collect()
    }

    pub(crate) fn code_mode(&self) -> Result<CodeMode> {
        let descriptors = self
            .operations
            .iter()
            .map(|operation| {
                let input_schema = Value::Object((*operation.tool.input_schema).clone());
                let output_schema = operation
                    .tool
                    .output_schema
                    .as_deref()
                    .map_or(Value::Bool(true), |schema| Value::Object(schema.clone()));
                FunctionDescriptor::from_schemas(
                    operation.name,
                    operation.description,
                    input_schema,
                    output_schema,
                )
                .map_err(|error| Error::CodeMode(error.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        CodeMode::new(descriptors).map_err(|error| Error::CodeMode(error.to_string()))
    }
}

fn operation<I, O>(
    name: &'static str,
    description: &'static str,
    access: Access,
    scope: Scope,
    idempotent: bool,
) -> Result<Operation>
where
    I: JsonSchema,
    O: JsonSchema + Serialize,
{
    let annotations = ToolAnnotations::new()
        .read_only(access == Access::Read)
        .destructive(access == Access::Destructive)
        .idempotent(idempotent)
        .open_world(scope != Scope::Global);
    let tool = Tool::new(name, description, json_object_schema::<I>()?)
        .with_raw_output_schema(json_object_schema::<O>()?)
        .with_annotations(annotations);
    Ok(Operation {
        name,
        description,
        access,
        scope,
        tool,
    })
}

pub(crate) fn json_object_schema<T: JsonSchema>() -> Result<Arc<JsonObject>> {
    let schema = serde_json::to_value(schemars::schema_for!(T))?;
    let Value::Object(object) = schema else {
        return Err(Error::InvalidConfig(
            "generated JSON Schema was not an object".to_owned(),
        ));
    };
    Ok(Arc::new(object))
}
