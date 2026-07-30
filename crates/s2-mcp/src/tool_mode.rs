use std::sync::Arc;

use rmcp::model::Tool;
use s2_mcp_codemode::{Limits, validate_json_depth};
use serde_json::Value;

use crate::{error::Result, operation_surface::OperationSurface};

#[derive(Clone)]
pub(crate) struct ToolMode {
    surface: Arc<OperationSurface>,
}

impl ToolMode {
    pub(crate) fn new(surface: Arc<OperationSurface>) -> Self {
        Self { surface }
    }

    pub(crate) fn tools(&self) -> Vec<Tool> {
        let mut tools = self.surface.tools();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    pub(crate) fn get_tool(&self, name: &str) -> Option<Tool> {
        self.surface.get_tool(name)
    }

    pub(crate) async fn dispatch(&self, name: &str, arguments: Value) -> Result<Value> {
        let maximum_depth = Limits::default().max_json_depth;
        validate_json_depth(&arguments, maximum_depth)
            .map_err(|error| crate::Error::CodeMode(error.to_string()))?;
        let result = self.surface.dispatch(name, arguments).await?;
        validate_json_depth(&result, maximum_depth)
            .map_err(|error| crate::Error::CodeMode(error.to_string()))?;
        Ok(result)
    }
}
