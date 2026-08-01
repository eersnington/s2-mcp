use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use s2_mcp_codemode::{CodeMode, FunctionDescriptor, SearchInput, SearchOutput};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{ConfigError, Error, Result},
    operation_registry,
    policy::{Access, Policy, Scope},
};

pub(crate) use crate::operation_registry::OperationId;

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
        let candidates = operation_registry::candidates(policy)?;
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
            .map_err(|error| Error::code_mode(error.to_string()))
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
            .map_err(|error| Error::code_mode(error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    CodeMode::new(descriptors).map_err(|error| Error::code_mode(error.to_string()))
}

pub(crate) fn operation<I, O>(
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
        return Err(Error::Config(ConfigError::Invalid(
            "generated JSON Schema was not an object".to_owned(),
        )));
    };
    Ok(Arc::new(object))
}
