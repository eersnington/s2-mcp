use std::sync::Arc;

use rmcp::model::Tool;
use s2_mcp_codemode::CodeMode;
use serde_json::Value;

use crate::{
    catalog::Catalog,
    config::ConnectionConfig,
    error::{Error, Result},
    operations::Operations,
    policy::Policy,
};

#[derive(Clone)]
pub(crate) struct OperationSurface {
    catalog: Catalog,
    operations: Operations,
    policy: Arc<Policy>,
}

impl OperationSurface {
    pub(crate) fn new(connection: ConnectionConfig, policy: Policy) -> Result<Self> {
        let policy = Arc::new(policy);
        Ok(Self {
            catalog: Catalog::new(&policy)?,
            operations: Operations::new(connection, policy.clone())?,
            policy,
        })
    }

    pub(crate) fn code_mode(&self) -> Result<CodeMode> {
        self.catalog.code_mode()
    }

    pub(crate) fn tools(&self) -> Vec<Tool> {
        self.catalog.tools()
    }

    pub(crate) fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog
            .find(name)
            .map(|operation| operation.tool.clone())
    }

    pub(crate) async fn dispatch(&self, name: &str, arguments: Value) -> Result<Value> {
        let id = self.catalog.find(name).ok_or(Error::Forbidden)?.id;
        self.operations.dispatch(id, arguments).await
    }

    pub(crate) fn policy(&self) -> &Policy {
        &self.policy
    }
}
