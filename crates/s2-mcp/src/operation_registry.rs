use serde_json::Value;

use crate::{
    catalog::Operation,
    error::Result,
    operations::Operations,
    policy::{Access, Policy, Scope},
};

macro_rules! operation_candidate {
    ($policy:expr, $id:expr, $description:expr, $input:ty, none, $output:ty, $idempotent:expr) => {
        $crate::catalog::operation::<$input, $output>($id, $description, $idempotent)?
    };
    ($policy:expr, $id:expr, $description:expr, $input:ty, [$basin_input:ty], $output:ty, $idempotent:expr) => {{
        if $policy.basin.is_some() {
            $crate::catalog::operation::<$basin_input, $output>($id, $description, $idempotent)?
        } else {
            $crate::catalog::operation::<$input, $output>($id, $description, $idempotent)?
        }
    }};
}

macro_rules! define_operation_registry {
    (
        $(
            $variant:ident {
                name: $name:literal,
                access: $access:expr,
                scope: $scope:expr,
                description: $description:literal,
                input: $input:ty,
                basin_input: $basin_input:tt,
                output: $output:ty,
                idempotent: $idempotent:literal,
                handler: $handler:ident
            }
        ),+ $(,)?
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum OperationId {
            $(
                $variant,
            )+
        }

        impl OperationId {
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            pub(crate) const fn access(self) -> Access {
                match self {
                    $(Self::$variant => $access,)+
                }
            }

            pub(crate) const fn scope(self) -> Scope {
                match self {
                    $(Self::$variant => $scope,)+
                }
            }

            pub(crate) const fn description(self) -> &'static str {
                match self {
                    $(Self::$variant => $description,)+
                }
            }

            pub(crate) const fn idempotent(self) -> bool {
                match self {
                    $(Self::$variant => $idempotent,)+
                }
            }
        }

        pub(crate) fn candidates(policy: &Policy) -> Result<Vec<Operation>> {
            Ok(vec![
                $(operation_candidate!(
                    policy,
                    OperationId::$variant,
                    OperationId::$variant.description(),
                    $input,
                    $basin_input,
                    $output,
                    OperationId::$variant.idempotent()
                ),)+
            ])
        }

        pub(crate) async fn dispatch(
            operations: &Operations,
            id: OperationId,
            arguments: Value,
        ) -> Result<Value> {
            match id {
                $(OperationId::$variant => operations.$handler(arguments).await,)+
            }
        }
    };
}

define_operation_registry! {
    ConnectionInfo {
        name: "connection_info",
        access: Access::Read,
        scope: Scope::Global,
        description: "Describe the active S2 endpoint and server policy without exposing credentials.",
        input: crate::operations::account::ConnectionInfoInput,
        basin_input: none,
        output: crate::operations::account::ConnectionInfoOutput,
        idempotent: true,
        handler: connection_info
    },
    ListBasins {
        name: "list_basins",
        access: Access::Read,
        scope: Scope::Account,
        description: "List a bounded page of basins in the S2 account.",
        input: crate::operations::account::ListBasinsRequest,
        basin_input: none,
        output: crate::operations::account::ListBasinsOutput,
        idempotent: true,
        handler: list_basins
    },
    GetBasinConfig {
        name: "get_basin_config",
        access: Access::Read,
        scope: Scope::Basin,
        description: "Get the effective configuration of an S2 basin.",
        input: crate::operations::account::GetBasinConfigRequest,
        basin_input: none,
        output: crate::operations::account::GetBasinConfigOutput,
        idempotent: true,
        handler: get_basin_config
    },
    ListStreams {
        name: "list_streams",
        access: Access::Read,
        scope: Scope::Basin,
        description: "List a bounded page of streams in an S2 basin.",
        input: crate::operations::basin::ListStreamsRequest,
        basin_input: none,
        output: crate::operations::basin::ListStreamsOutput,
        idempotent: true,
        handler: list_streams
    },
    GetStreamConfig {
        name: "get_stream_config",
        access: Access::Read,
        scope: Scope::Stream,
        description: "Get the effective configuration of an S2 stream.",
        input: crate::operations::basin::GetStreamConfigRequest,
        basin_input: none,
        output: crate::operations::basin::GetStreamConfigOutput,
        idempotent: true,
        handler: get_stream_config
    },
    CheckTail {
        name: "check_tail",
        access: Access::Read,
        scope: Scope::Stream,
        description: "Get the current tail sequence number and timestamp for an S2 stream.",
        input: crate::operations::stream::StreamRequest,
        basin_input: none,
        output: crate::operations::stream::PositionOutput,
        idempotent: true,
        handler: check_tail
    },
    ReadRecords {
        name: "read_records",
        access: Access::Read,
        scope: Scope::Stream,
        description: "Read a bounded batch of records from an S2 stream without waiting.",
        input: crate::operations::records::ReadRecordsRequest,
        basin_input: none,
        output: crate::operations::records::ReadRecordsOutput,
        idempotent: true,
        handler: read_records
    },
    WaitForRecords {
        name: "wait_for_records",
        access: Access::Read,
        scope: Scope::Stream,
        description: "Wait for and read a bounded batch of records from an S2 stream.",
        input: crate::operations::records::WaitForRecordsRequest,
        basin_input: none,
        output: crate::operations::records::WaitForRecordsOutput,
        idempotent: true,
        handler: wait_for_records
    },
    DiffResources {
        name: "diff_resources",
        access: Access::Read,
        scope: Scope::Dynamic {
            applicable_under_basin: true,
        },
        description: "Compare desired basin or stream configurations with their current S2 state.",
        input: crate::operations::basin::DiffResourcesRequest,
        basin_input: none,
        output: crate::operations::basin::DiffResourcesOutput,
        idempotent: true,
        handler: diff_resources
    },
    GetMetrics {
        name: "get_metrics",
        access: Access::Read,
        scope: Scope::Dynamic {
            applicable_under_basin: true,
        },
        description: "Get bounded account, basin, or stream metrics from S2.",
        input: crate::operations::account::GetMetricsRequest,
        basin_input: [crate::operations::account::BasinScopedGetMetricsSchema],
        output: crate::operations::account::MetricsOutput,
        idempotent: true,
        handler: get_metrics
    },
    EnsureBasin {
        name: "ensure_basin",
        access: Access::Write,
        scope: Scope::Basin,
        description: "Create an S2 basin or update it to the requested configuration.",
        input: crate::operations::account::EnsureBasinRequest,
        basin_input: none,
        output: crate::operations::account::EnsureBasinOutput,
        idempotent: true,
        handler: ensure_basin
    },
    EnsureStream {
        name: "ensure_stream",
        access: Access::Write,
        scope: Scope::Stream,
        description: "Create an S2 stream or update it to the requested configuration.",
        input: crate::operations::basin::EnsureStreamRequest,
        basin_input: none,
        output: crate::operations::basin::EnsureStreamOutput,
        idempotent: true,
        handler: ensure_stream
    },
    AppendRecords {
        name: "append_records",
        access: Access::Write,
        scope: Scope::Stream,
        description: "Atomically append a bounded batch of records to an S2 stream.",
        input: crate::operations::records::AppendRecordsRequest,
        basin_input: none,
        output: crate::operations::records::AppendRecordsOutput,
        idempotent: false,
        handler: append_records
    },
    ReconfigureBasin {
        name: "reconfigure_basin",
        access: Access::Write,
        scope: Scope::Basin,
        description: "Apply a partial configuration update to an S2 basin.",
        input: crate::operations::account::ReconfigureBasinRequest,
        basin_input: none,
        output: crate::operations::account::GetBasinConfigOutput,
        idempotent: true,
        handler: reconfigure_basin
    },
    ReconfigureStream {
        name: "reconfigure_stream",
        access: Access::Write,
        scope: Scope::Stream,
        description: "Apply a partial configuration update to an S2 stream.",
        input: crate::operations::basin::ReconfigureStreamRequest,
        basin_input: none,
        output: crate::operations::basin::GetStreamConfigOutput,
        idempotent: true,
        handler: reconfigure_stream
    },
    FenceStream {
        name: "fence_stream",
        access: Access::Write,
        scope: Scope::Stream,
        description: "Set or clear the fencing token on an S2 stream.",
        input: crate::operations::stream::FenceStreamRequest,
        basin_input: none,
        output: crate::operations::stream::StreamCommandOutput,
        idempotent: false,
        handler: fence_stream
    },
    DeleteBasin {
        name: "delete_basin",
        access: Access::Destructive,
        scope: Scope::Basin,
        description: "Delete an S2 basin.",
        input: crate::operations::account::DeleteBasinRequest,
        basin_input: none,
        output: crate::operations::basin::DeleteResourceOutput,
        idempotent: true,
        handler: delete_basin
    },
    DeleteStream {
        name: "delete_stream",
        access: Access::Destructive,
        scope: Scope::Stream,
        description: "Delete an S2 stream.",
        input: crate::operations::basin::DeleteStreamRequest,
        basin_input: none,
        output: crate::operations::basin::DeleteResourceOutput,
        idempotent: true,
        handler: delete_stream
    },
    TrimStream {
        name: "trim_stream",
        access: Access::Destructive,
        scope: Scope::Stream,
        description: "Advance the earliest retained sequence number of an S2 stream.",
        input: crate::operations::stream::TrimStreamRequest,
        basin_input: none,
        output: crate::operations::stream::StreamCommandOutput,
        idempotent: false,
        handler: trim_stream
    },
    RevokeAccessToken {
        name: "revoke_access_token",
        access: Access::Destructive,
        scope: Scope::Account,
        description: "Revoke an S2 access token by ID.",
        input: crate::operations::account::RevokeAccessTokenRequest,
        basin_input: none,
        output: crate::operations::account::RevokeAccessTokenOutput,
        idempotent: true,
        handler: revoke_access_token
    },
}
