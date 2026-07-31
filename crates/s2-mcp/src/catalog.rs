use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use s2_mcp_codemode::{CodeMode, FunctionDescriptor, SearchInput, SearchOutput};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{Error, Result},
    operations::{
        account::{
            BasinScopedGetMetricsSchema, ConnectionInfoInput, ConnectionInfoOutput,
            DeleteBasinRequest, EnsureBasinOutput, EnsureBasinRequest, GetBasinConfigOutput,
            GetBasinConfigRequest, GetMetricsRequest, ListBasinsOutput, ListBasinsRequest,
            MetricsOutput, ReconfigureBasinRequest, RevokeAccessTokenOutput,
            RevokeAccessTokenRequest,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationId {
    ConnectionInfo,
    ListBasins,
    GetBasinConfig,
    ListStreams,
    GetStreamConfig,
    CheckTail,
    ReadRecords,
    WaitForRecords,
    DiffResources,
    GetMetrics,
    EnsureBasin,
    EnsureStream,
    AppendRecords,
    ReconfigureBasin,
    ReconfigureStream,
    FenceStream,
    DeleteBasin,
    DeleteStream,
    TrimStream,
    RevokeAccessToken,
}

impl OperationId {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ConnectionInfo => "connection_info",
            Self::ListBasins => "list_basins",
            Self::GetBasinConfig => "get_basin_config",
            Self::ListStreams => "list_streams",
            Self::GetStreamConfig => "get_stream_config",
            Self::CheckTail => "check_tail",
            Self::ReadRecords => "read_records",
            Self::WaitForRecords => "wait_for_records",
            Self::DiffResources => "diff_resources",
            Self::GetMetrics => "get_metrics",
            Self::EnsureBasin => "ensure_basin",
            Self::EnsureStream => "ensure_stream",
            Self::AppendRecords => "append_records",
            Self::ReconfigureBasin => "reconfigure_basin",
            Self::ReconfigureStream => "reconfigure_stream",
            Self::FenceStream => "fence_stream",
            Self::DeleteBasin => "delete_basin",
            Self::DeleteStream => "delete_stream",
            Self::TrimStream => "trim_stream",
            Self::RevokeAccessToken => "revoke_access_token",
        }
    }

    pub(crate) const fn access(self) -> Access {
        match self {
            Self::ConnectionInfo
            | Self::ListBasins
            | Self::GetBasinConfig
            | Self::ListStreams
            | Self::GetStreamConfig
            | Self::CheckTail
            | Self::ReadRecords
            | Self::WaitForRecords
            | Self::DiffResources
            | Self::GetMetrics => Access::Read,
            Self::EnsureBasin
            | Self::EnsureStream
            | Self::AppendRecords
            | Self::ReconfigureBasin
            | Self::ReconfigureStream
            | Self::FenceStream => Access::Write,
            Self::DeleteBasin | Self::DeleteStream | Self::TrimStream | Self::RevokeAccessToken => {
                Access::Destructive
            }
        }
    }

    pub(crate) const fn scope(self) -> Scope {
        match self {
            Self::ConnectionInfo => Scope::Global,
            Self::ListBasins | Self::RevokeAccessToken => Scope::Account,
            Self::GetBasinConfig
            | Self::ListStreams
            | Self::EnsureBasin
            | Self::ReconfigureBasin
            | Self::DeleteBasin => Scope::Basin,
            Self::GetStreamConfig
            | Self::CheckTail
            | Self::ReadRecords
            | Self::WaitForRecords
            | Self::EnsureStream
            | Self::AppendRecords
            | Self::ReconfigureStream
            | Self::FenceStream
            | Self::DeleteStream
            | Self::TrimStream => Scope::Stream,
            Self::DiffResources | Self::GetMetrics => Scope::Dynamic {
                applicable_under_basin: true,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Operation {
    pub(crate) id: OperationId,
    pub(crate) description: &'static str,
    pub(crate) tool: Tool,
}

#[derive(Debug, Clone)]
pub(crate) struct Catalog {
    operations: Arc<[Operation]>,
    code_mode: CodeMode,
}

impl Catalog {
    pub(crate) fn new(policy: &Policy) -> Result<Self> {
        let get_metrics = if policy.basin.is_some() {
            operation::<BasinScopedGetMetricsSchema, MetricsOutput>(
                OperationId::GetMetrics,
                "Get bounded account, basin, or stream metrics from S2.",
                true,
            )?
        } else {
            operation::<GetMetricsRequest, MetricsOutput>(
                OperationId::GetMetrics,
                "Get bounded account, basin, or stream metrics from S2.",
                true,
            )?
        };
        let candidates = [
            operation::<ConnectionInfoInput, ConnectionInfoOutput>(
                OperationId::ConnectionInfo,
                "Describe the active S2 endpoint and server policy without exposing credentials.",
                true,
            )?,
            operation::<ListBasinsRequest, ListBasinsOutput>(
                OperationId::ListBasins,
                "List a bounded page of basins in the S2 account.",
                true,
            )?,
            operation::<GetBasinConfigRequest, GetBasinConfigOutput>(
                OperationId::GetBasinConfig,
                "Get the effective configuration of an S2 basin.",
                true,
            )?,
            operation::<ListStreamsRequest, ListStreamsOutput>(
                OperationId::ListStreams,
                "List a bounded page of streams in an S2 basin.",
                true,
            )?,
            operation::<GetStreamConfigRequest, GetStreamConfigOutput>(
                OperationId::GetStreamConfig,
                "Get the effective configuration of an S2 stream.",
                true,
            )?,
            operation::<StreamRequest, PositionOutput>(
                OperationId::CheckTail,
                "Get the current tail sequence number and timestamp for an S2 stream.",
                true,
            )?,
            operation::<ReadRecordsRequest, ReadRecordsOutput>(
                OperationId::ReadRecords,
                "Read a bounded batch of records from an S2 stream without waiting.",
                true,
            )?,
            operation::<WaitForRecordsRequest, WaitForRecordsOutput>(
                OperationId::WaitForRecords,
                "Wait for and read a bounded batch of records from an S2 stream.",
                true,
            )?,
            operation::<DiffResourcesRequest, DiffResourcesOutput>(
                OperationId::DiffResources,
                "Compare desired basin or stream configurations with their current S2 state.",
                true,
            )?,
            get_metrics,
            operation::<EnsureBasinRequest, EnsureBasinOutput>(
                OperationId::EnsureBasin,
                "Create an S2 basin or update it to the requested configuration.",
                true,
            )?,
            operation::<EnsureStreamRequest, EnsureStreamOutput>(
                OperationId::EnsureStream,
                "Create an S2 stream or update it to the requested configuration.",
                true,
            )?,
            operation::<AppendRecordsRequest, AppendRecordsOutput>(
                OperationId::AppendRecords,
                "Atomically append a bounded batch of records to an S2 stream.",
                false,
            )?,
            operation::<ReconfigureBasinRequest, GetBasinConfigOutput>(
                OperationId::ReconfigureBasin,
                "Apply a partial configuration update to an S2 basin.",
                true,
            )?,
            operation::<ReconfigureStreamRequest, GetStreamConfigOutput>(
                OperationId::ReconfigureStream,
                "Apply a partial configuration update to an S2 stream.",
                true,
            )?,
            operation::<FenceStreamRequest, StreamCommandOutput>(
                OperationId::FenceStream,
                "Set or clear the fencing token on an S2 stream.",
                false,
            )?,
            operation::<DeleteBasinRequest, DeleteResourceOutput>(
                OperationId::DeleteBasin,
                "Delete an S2 basin.",
                true,
            )?,
            operation::<DeleteStreamRequest, DeleteResourceOutput>(
                OperationId::DeleteStream,
                "Delete an S2 stream.",
                true,
            )?,
            operation::<TrimStreamRequest, StreamCommandOutput>(
                OperationId::TrimStream,
                "Advance the earliest retained sequence number of an S2 stream.",
                false,
            )?,
            operation::<RevokeAccessTokenRequest, RevokeAccessTokenOutput>(
                OperationId::RevokeAccessToken,
                "Revoke an S2 access token by ID.",
                true,
            )?,
        ];
        let operations: Arc<[Operation]> = candidates
            .into_iter()
            .filter(|operation| policy.allows(operation.id.access(), operation.id.scope()))
            .collect::<Vec<_>>()
            .into();
        let code_mode = build_code_mode(&operations)?;
        Ok(Self {
            operations,
            code_mode,
        })
    }

    pub(crate) fn find(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|operation| operation.id.name() == name)
    }

    pub(crate) fn tools(&self) -> Vec<Tool> {
        self.operations
            .iter()
            .map(|operation| operation.tool.clone())
            .collect()
    }

    pub(crate) fn code_mode(&self) -> CodeMode {
        self.code_mode.clone()
    }

    pub(crate) fn search(&self, input: SearchInput) -> Result<SearchOutput> {
        self.code_mode
            .search(input)
            .map_err(|error| Error::CodeMode(error.to_string()))
    }
}

fn build_code_mode(operations: &[Operation]) -> Result<CodeMode> {
    let descriptors = operations
        .iter()
        .map(|operation| {
            let input_schema = Value::Object((*operation.tool.input_schema).clone());
            let output_schema = operation
                .tool
                .output_schema
                .as_deref()
                .map_or(Value::Bool(true), |schema| Value::Object(schema.clone()));
            FunctionDescriptor::from_schemas(
                operation.id.name(),
                operation.description,
                input_schema,
                output_schema,
            )
            .map_err(|error| Error::CodeMode(error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    CodeMode::new(descriptors).map_err(|error| Error::CodeMode(error.to_string()))
}

fn operation<I, O>(
    id: OperationId,
    description: &'static str,
    idempotent: bool,
) -> Result<Operation>
where
    I: JsonSchema,
    O: JsonSchema + Serialize,
{
    let annotations = ToolAnnotations::new()
        .read_only(id.access() == Access::Read)
        .destructive(id.access() == Access::Destructive)
        .idempotent(idempotent)
        .open_world(id.scope() != Scope::Global);
    let tool = Tool::new(id.name(), description, json_object_schema::<I>()?)
        .with_raw_output_schema(json_object_schema::<O>()?)
        .with_annotations(annotations);
    Ok(Operation {
        id,
        description,
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
