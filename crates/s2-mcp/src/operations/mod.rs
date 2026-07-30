use std::sync::Arc;

use s2_sdk::{
    S2,
    types::{EncryptionKey, S2DateTime},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    catalog::Catalog,
    config::ConnectionConfig,
    error::{Error, Result},
    policy::Policy,
};

pub(crate) mod account;
pub(crate) mod basin;
pub(crate) mod records;
pub(crate) mod stream;

pub(crate) type SharedOperationHandler = Arc<Operations>;

pub(crate) async fn dispatch(
    handler: &Operations,
    catalog: &Catalog,
    policy: &Policy,
    name: &str,
    arguments: Value,
) -> Result<Value> {
    let operation = catalog.find(name).ok_or(Error::Forbidden)?;
    policy.enforce_operation(operation.access, operation.scope)?;
    handler.dispatch(name, arguments).await
}

#[derive(Clone)]
pub(crate) struct Operations {
    pub(super) s2: S2,
    pub(super) connection: ConnectionConfig,
    pub(super) encryption_key: Option<EncryptionKey>,
    pub(super) policy: Policy,
}

impl Operations {
    pub(crate) fn new(connection: ConnectionConfig, policy: Policy) -> Result<Self> {
        let encryption_key = connection.encryption_key()?;
        let s2 = S2::new(connection.sdk_config()?)?;
        Ok(Self {
            s2,
            connection,
            encryption_key,
            policy,
        })
    }

    pub(crate) async fn dispatch(&self, name: &str, arguments: Value) -> Result<Value> {
        match name {
            "connection_info" => self.connection_info(arguments),
            "list_basins" => self.list_basins(arguments).await,
            "get_basin_config" => self.get_basin_config(arguments).await,
            "list_streams" => self.list_streams(arguments).await,
            "get_stream_config" => self.get_stream_config(arguments).await,
            "check_tail" => self.check_tail(arguments).await,
            "read_records" => self.read_records(arguments).await,
            "wait_for_records" => self.wait_for_records(arguments).await,
            "diff_resources" => self.diff_resources(arguments).await,
            "get_metrics" => self.get_metrics(arguments).await,
            "ensure_basin" => self.ensure_basin(arguments).await,
            "ensure_stream" => self.ensure_stream(arguments).await,
            "append_records" => self.append_records(arguments).await,
            "reconfigure_basin" => self.reconfigure_basin(arguments).await,
            "reconfigure_stream" => self.reconfigure_stream(arguments).await,
            "fence_stream" => self.fence_stream(arguments).await,
            "delete_basin" => self.delete_basin(arguments).await,
            "delete_stream" => self.delete_stream(arguments).await,
            "trim_stream" => self.trim_stream(arguments).await,
            "revoke_access_token" => self.revoke_access_token(arguments).await,
            _ => Err(Error::Forbidden),
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
