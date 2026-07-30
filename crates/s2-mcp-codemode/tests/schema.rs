use s2_mcp_codemode::{Error, FunctionDescriptor};
use serde_json::json;

#[test]
fn rejects_required_property_absent_from_properties() {
    let result = FunctionDescriptor::from_schemas(
        "test_operation",
        "Test operation.",
        json!({
            "type": "object",
            "properties": {"present": {"type": "string"}},
            "required": ["missing"]
        }),
        json!({}),
    );

    assert!(matches!(
        result,
        Err(Error::InvalidDescriptor(message)) if message.contains("`missing`")
    ));
}

#[test]
fn rejects_typed_additional_properties_with_declared_properties() {
    let result = FunctionDescriptor::from_schemas(
        "test_operation",
        "Test operation.",
        json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "additionalProperties": {"type": "number"}
        }),
        json!({}),
    );

    assert!(matches!(result, Err(Error::InvalidDescriptor(_))));
}
