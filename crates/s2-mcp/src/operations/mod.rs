use std::sync::Arc;

use s2_sdk::{
    S2,
    types::{EncryptionKey, S2DateTime},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    catalog::OperationId,
    config::ConnectionConfig,
    error::{Error, Result},
    policy::Policy,
};

pub(crate) mod account;
pub(crate) mod basin;
pub(crate) mod records;
pub(crate) mod stream;

#[derive(Clone)]
pub(crate) struct Operations {
    pub(super) s2: S2,
    pub(super) connection: ConnectionConfig,
    pub(super) encryption_key: Option<EncryptionKey>,
    pub(super) policy: Arc<Policy>,
}

impl Operations {
    pub(crate) fn new(connection: ConnectionConfig, policy: Arc<Policy>) -> Result<Self> {
        let encryption_key = connection.encryption_key()?;
        let s2 = S2::new(connection.sdk_config()?)?;
        Ok(Self {
            s2,
            connection,
            encryption_key,
            policy,
        })
    }

    pub(crate) async fn dispatch(&self, id: OperationId, arguments: Value) -> Result<Value> {
        match id {
            OperationId::ConnectionInfo => self.connection_info(arguments),
            OperationId::ListBasins => self.list_basins(arguments).await,
            OperationId::GetBasinConfig => self.get_basin_config(arguments).await,
            OperationId::ListStreams => self.list_streams(arguments).await,
            OperationId::GetStreamConfig => self.get_stream_config(arguments).await,
            OperationId::CheckTail => self.check_tail(arguments).await,
            OperationId::ReadRecords => self.read_records(arguments).await,
            OperationId::WaitForRecords => self.wait_for_records(arguments).await,
            OperationId::DiffResources => self.diff_resources(arguments).await,
            OperationId::GetMetrics => self.get_metrics(arguments).await,
            OperationId::EnsureBasin => self.ensure_basin(arguments).await,
            OperationId::EnsureStream => self.ensure_stream(arguments).await,
            OperationId::AppendRecords => self.append_records(arguments).await,
            OperationId::ReconfigureBasin => self.reconfigure_basin(arguments).await,
            OperationId::ReconfigureStream => self.reconfigure_stream(arguments).await,
            OperationId::FenceStream => self.fence_stream(arguments).await,
            OperationId::DeleteBasin => self.delete_basin(arguments).await,
            OperationId::DeleteStream => self.delete_stream(arguments).await,
            OperationId::TrimStream => self.trim_stream(arguments).await,
            OperationId::RevokeAccessToken => self.revoke_access_token(arguments).await,
        }
    }
}

pub(super) fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|error| Error::InvalidArguments(error.to_string()))
}

pub(super) fn serialize<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

pub(super) fn bounded(value: usize, maximum: usize, name: &str) -> Result<usize> {
    if value == 0 || value > maximum {
        return Err(Error::InvalidArguments(format!(
            "{name} must be between 1 and {maximum}"
        )));
    }
    Ok(value)
}

pub(super) fn date_time(value: S2DateTime) -> String {
    value.to_string()
}
