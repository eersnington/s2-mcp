use std::sync::Arc;

use s2_sdk::{
    S2,
    types::{EncryptionKey, S2DateTime},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::{
    config::ConnectionConfig,
    error::{Error, Result},
    operation_registry::{self, OperationId},
    policy::Policy,
};

pub(crate) mod account;
pub(crate) mod basin;
pub(crate) mod records;
pub(crate) mod stream;

pub(crate) struct Operations {
    pub(super) s2: OnceCell<S2>,
    pub(super) connection: ConnectionConfig,
    pub(super) encryption_key: Option<EncryptionKey>,
    pub(super) policy: Arc<Policy>,
}

impl Operations {
    pub(crate) fn new(connection: ConnectionConfig, policy: Arc<Policy>) -> Result<Self> {
        let encryption_key = connection.encryption_key()?;
        Ok(Self {
            s2: OnceCell::new(),
            connection,
            encryption_key,
            policy,
        })
    }

    pub(super) async fn s2(&self) -> Result<&S2> {
        self.s2
            .get_or_try_init(|| async { Ok(S2::new(self.connection.sdk_config()?)?) })
            .await
    }

    pub(crate) async fn dispatch(&self, id: OperationId, arguments: Value) -> Result<Value> {
        self.policy.enforce_operation(id.access(), id.scope())?;
        let mut arguments = arguments;
        if let Some(basin) = &self.policy.basin
            && id.scope().accepts_basin_injection()
            && let Value::Object(object) = &mut arguments
        {
            object
                .entry("basin")
                .or_insert_with(|| Value::String(basin.clone()));
        }
        operation_registry::dispatch(self, id, arguments).await
    }
}

pub(super) fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|error| Error::invalid_arguments(error.to_string()))
}

pub(super) fn serialize<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

pub(super) fn bounded(value: usize, maximum: usize, name: &str) -> Result<usize> {
    if value == 0 || value > maximum {
        return Err(Error::invalid_arguments(format!(
            "{name} must be between 1 and {maximum}"
        )));
    }
    Ok(value)
}

pub(super) fn date_time(value: S2DateTime) -> String {
    value.to_string()
}
