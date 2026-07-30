use rmcp::model::Tool;
use s2_mcp_codemode::{Limits, validate_json_depth};
use serde_json::Value;

use crate::{
    catalog::Catalog,
    error::Result,
    operations::{SharedOperationHandler, dispatch},
    policy::Policy,
};

#[derive(Clone)]
pub(crate) struct ToolMode {
    catalog: Catalog,
    handler: SharedOperationHandler,
    policy: Policy,
}

impl ToolMode {
    pub(crate) fn new(catalog: Catalog, handler: SharedOperationHandler, policy: Policy) -> Self {
        Self {
            catalog,
            handler,
            policy,
        }
    }

    pub(crate) fn tools(&self) -> Vec<Tool> {
        let mut tools = self.catalog.tools();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    pub(crate) fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog
            .find(name)
            .map(|operation| operation.tool.clone())
    }

    pub(crate) async fn dispatch(&self, name: &str, arguments: Value) -> Result<Value> {
        let maximum_depth = Limits::default().max_json_depth;
        validate_json_depth(&arguments, maximum_depth)
            .map_err(|error| crate::Error::CodeMode(error.to_string()))?;
        let result = dispatch(&self.handler, &self.catalog, &self.policy, name, arguments).await?;
        validate_json_depth(&result, maximum_depth)
            .map_err(|error| crate::Error::CodeMode(error.to_string()))?;
        Ok(result)
    }
}
