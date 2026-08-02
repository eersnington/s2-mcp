use std::{collections::BTreeMap, sync::Arc};

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use s2_mcp_codemode::{CodeMode, ExecutionApi, FunctionDescriptor, SearchInput, SearchOutput};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::operation_registry::OperationId;
use crate::{
    error::{ConfigError, Error, Result},
    operation_registry,
    policy::{Access, Policy, Scope},
};

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
        let mut operations = candidates
            .into_iter()
            .filter(|operation| policy.allows(operation.id.access(), operation.id.scope()))
            .collect::<Vec<_>>();
        if policy.basin.is_some() {
            for operation in &mut operations {
                if operation.id.scope().accepts_basin_injection() {
                    remove_basin_input(operation);
                }
            }
        }
        let operations: Arc<[Operation]> = operations.into();
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

    pub(crate) fn execution_manifest(&self) -> (ExecutionApi, BTreeMap<String, OperationId>) {
        let operation_ids = self
            .operations
            .iter()
            .map(|operation| (operation.id.name().to_owned(), operation.id))
            .collect();
        (self.code_mode.execution_api(), operation_ids)
    }

    pub(crate) fn search(&self, input: SearchInput) -> Result<SearchOutput> {
        self.code_mode.search(input).map_err(Into::into)
    }
}

fn remove_basin_input(operation: &mut Operation) {
    let schema = Arc::make_mut(&mut operation.tool.input_schema);
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.remove("basin");
    }
    if let Some(Value::Array(required)) = schema.get_mut("required") {
        required.retain(|name| name != "basin");
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
            .map_err(Error::from)
        })
        .collect::<Result<Vec<_>>>()?;
    CodeMode::new(descriptors).map_err(Into::into)
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
