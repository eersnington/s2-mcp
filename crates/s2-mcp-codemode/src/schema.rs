use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::{Error, FunctionDescriptor};

const NAMESPACE: &str = "S2";

type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn generate_function(
    operation: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
) -> Result<FunctionDescriptor, Error> {
    let function_name = lower_camel_case(operation)?;
    let type_prefix = upper_camel_case(operation)?;
    let input_type_name = format!("{type_prefix}Input");
    let output_type_name = format!("{type_prefix}Output");
    let input_type = SchemaRenderer::new(&input_schema).render()?;
    let output_type = SchemaRenderer::new(&output_schema).render()?;
    let input_default = schema_allows_empty_object(&input_schema);
    let public_name = format!("{NAMESPACE}.{function_name}");

    let declaration_body = function_body(
        description,
        &function_name,
        &input_type_name,
        &input_type,
        &output_type_name,
        &output_type,
        input_default,
    );
    let signature = format!("declare namespace {NAMESPACE} {{\n{declaration_body}\n}}");

    Ok(FunctionDescriptor {
        operation: operation.to_owned(),
        name: public_name,
        description: description.to_owned(),
        signature,
        input_schema,
    })
}

fn function_body(
    description: &str,
    function_name: &str,
    input_type_name: &str,
    input_type: &str,
    output_type_name: &str,
    output_type: &str,
    input_default: bool,
) -> String {
    let description = doc_comment(description, "  ");
    let default_value = if input_default { " = {}" } else { "" };
    let declaration = format!(
        "export function {function_name}(input: {input_type_name}{default_value}): Promise<{output_type_name}>;"
    );
    format!(
        "  export type {input_type_name} = {input_type};\n\n  export type {output_type_name} = {output_type};\n\n{description}\n  {declaration}"
    )
}

fn schema_allows_empty_object(root: &Value) -> bool {
    schema_value_allows_empty_object(root, root, &mut BTreeSet::new())
}

fn schema_value_allows_empty_object(
    root: &Value,
    schema: &Value,
    resolving: &mut BTreeSet<String>,
) -> bool {
    let Some(schema) = schema.as_object() else {
        return schema == &Value::Bool(true);
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let Some(pointer) = reference.strip_prefix('#') else {
            return false;
        };
        if !resolving.insert(reference.to_owned()) {
            return false;
        }
        let result = root
            .pointer(pointer)
            .is_some_and(|resolved| schema_value_allows_empty_object(root, resolved, resolving));
        resolving.remove(reference);
        return result;
    }
    for keyword in ["oneOf", "anyOf"] {
        if let Some(variants) = schema.get(keyword).and_then(Value::as_array) {
            return variants
                .iter()
                .any(|variant| schema_value_allows_empty_object(root, variant, resolving));
        }
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        return parts
            .iter()
            .all(|part| schema_value_allows_empty_object(root, part, resolving));
    }
    if schema.contains_key("const") || schema.contains_key("enum") {
        return false;
    }
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

fn lower_camel_case(name: &str) -> Result<String, Error> {
    let mut segments = name.split('_');
    let first = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            Error::InvalidDescriptor(
                "operation name cannot be empty when generating TypeScript".to_owned(),
            )
        })?;
    let mut output = first.to_owned();
    for segment in segments {
        push_capitalized(&mut output, segment, name)?;
    }
    Ok(output)
}

fn upper_camel_case(name: &str) -> Result<String, Error> {
    let mut output = String::new();
    for segment in name.split('_') {
        push_capitalized(&mut output, segment, name)?;
    }
    Ok(output)
}

fn push_capitalized(output: &mut String, segment: &str, original: &str) -> Result<(), Error> {
    let mut characters = segment.chars();
    let Some(first) = characters.next() else {
        return Err(Error::InvalidDescriptor(format!(
            "operation name `{original}` cannot be converted to TypeScript"
        )));
    };
    output.push(first.to_ascii_uppercase());
    output.extend(characters);
    Ok(())
}

fn doc_comment(description: &str, indentation: &str) -> String {
    let safe = description.replace("*/", "* /");
    let lines = safe
        .lines()
        .map(|line| format!("{indentation} * {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{indentation}/**\n{lines}\n{indentation} */")
}

struct SchemaRenderer<'a> {
    root: &'a Value,
    resolving: BTreeSet<String>,
}

impl<'a> SchemaRenderer<'a> {
    fn new(root: &'a Value) -> Self {
        Self {
            root,
            resolving: BTreeSet::new(),
        }
    }

    fn render(mut self) -> Result<String> {
        let root = self.root;
        self.render_schema(root)
    }

    fn render_schema(&mut self, schema: &Value) -> Result<String> {
        match schema {
            Value::Bool(true) => Ok("unknown".to_owned()),
            Value::Bool(false) => Ok("never".to_owned()),
            Value::Object(object) => self.render_object_schema(object),
            _ => Err(Error::InvalidDescriptor(
                "operation JSON Schema was not an object or boolean".to_owned(),
            )),
        }
    }

    fn render_object_schema(&mut self, schema: &Map<String, Value>) -> Result<String> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            return self.render_reference(reference);
        }
        if let Some(value) = schema.get("const") {
            return literal_type(value);
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return render_literals(values);
        }

        for keyword in ["oneOf", "anyOf"] {
            if let Some(variants) = schema.get(keyword).and_then(Value::as_array) {
                return self.render_union(variants);
            }
        }
        if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
            return self.render_intersection(parts);
        }

        match schema.get("type") {
            Some(Value::String(schema_type)) => self.render_type(schema_type, schema),
            Some(Value::Array(schema_types)) => {
                let mut rendered = Vec::with_capacity(schema_types.len());
                for schema_type in schema_types {
                    let Some(schema_type) = schema_type.as_str() else {
                        return Err(Error::InvalidDescriptor(
                            "operation JSON Schema contains a non-string type".to_owned(),
                        ));
                    };
                    rendered.push(self.render_type(schema_type, schema)?);
                }
                Ok(join_types(rendered, " | "))
            }
            None if schema.contains_key("properties")
                || schema.contains_key("additionalProperties") =>
            {
                self.render_object_type(schema)
            }
            None => Ok("unknown".to_owned()),
            _ => Err(Error::InvalidDescriptor(
                "operation JSON Schema contains an invalid type".to_owned(),
            )),
        }
    }

    fn render_reference(&mut self, reference: &str) -> Result<String> {
        let Some(pointer) = reference.strip_prefix('#') else {
            return Err(Error::InvalidDescriptor(format!(
                "operation JSON Schema contains unsupported external reference `{reference}`"
            )));
        };
        if !self.resolving.insert(reference.to_owned()) {
            return Err(Error::InvalidDescriptor(format!(
                "operation JSON Schema contains recursive reference `{reference}`"
            )));
        }
        let schema = self.root.pointer(pointer).ok_or_else(|| {
            Error::InvalidDescriptor(format!(
                "operation JSON Schema reference `{reference}` could not be resolved"
            ))
        })?;
        let rendered = self.render_schema(schema);
        self.resolving.remove(reference);
        rendered
    }

    fn render_union(&mut self, variants: &[Value]) -> Result<String> {
        let mut rendered = Vec::with_capacity(variants.len());
        for variant in variants {
            rendered.push(self.render_schema(variant)?);
        }
        Ok(join_types(rendered, " | "))
    }

    fn render_intersection(&mut self, parts: &[Value]) -> Result<String> {
        let mut rendered = Vec::with_capacity(parts.len());
        for part in parts {
            rendered.push(self.render_schema(part)?);
        }
        Ok(join_types(rendered, " & "))
    }

    fn render_type(&mut self, schema_type: &str, schema: &Map<String, Value>) -> Result<String> {
        match schema_type {
            "null" => Ok("null".to_owned()),
            "boolean" => Ok("boolean".to_owned()),
            "integer" | "number" => Ok("number".to_owned()),
            "string" => Ok("string".to_owned()),
            "array" => {
                let item_type = match schema.get("items") {
                    Some(items) => self.render_schema(items)?,
                    None => "unknown".to_owned(),
                };
                Ok(format!("Array<{item_type}>"))
            }
            "object" => self.render_object_type(schema),
            other => Err(Error::InvalidDescriptor(format!(
                "operation JSON Schema contains unsupported type `{other}`"
            ))),
        }
    }

    fn render_object_type(&mut self, schema: &Map<String, Value>) -> Result<String> {
        let properties = schema.get("properties").and_then(Value::as_object);
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if let Some(name) = required
            .iter()
            .find(|name| properties.is_none_or(|properties| !properties.contains_key(**name)))
        {
            return Err(Error::InvalidDescriptor(format!(
                "operation JSON Schema requires property `{name}`, but it is absent from `properties`"
            )));
        }
        if properties.is_some_and(|properties| !properties.is_empty())
            && schema
                .get("additionalProperties")
                .is_some_and(Value::is_object)
        {
            return Err(Error::InvalidDescriptor(
                "operation JSON Schema cannot combine declared properties with typed `additionalProperties`"
                    .to_owned(),
            ));
        }
        let mut fields = Vec::new();
        if let Some(properties) = properties {
            for (name, property_schema) in properties {
                let property_type = self.render_schema(property_schema)?;
                let optional = if required.contains(name.as_str()) {
                    ""
                } else {
                    "?"
                };
                let description = property_schema
                    .as_object()
                    .and_then(|schema| schema.get("description"))
                    .and_then(Value::as_str)
                    .map(|description| format!("\n{}\n", doc_comment(description, "    ")))
                    .unwrap_or_else(|| "\n".to_owned());
                let name = serde_json::to_string(name).map_err(Error::Serialize)?;
                fields.push(format!(
                    "{description}    {name}{optional}: {property_type};"
                ));
            }
        }

        let object = if fields.is_empty() {
            "{}".to_owned()
        } else {
            format!("{{{}\n  }}", fields.join(""))
        };
        match schema.get("additionalProperties") {
            Some(Value::Object(additional)) => {
                let additional = self.render_schema(&Value::Object(additional.clone()))?;
                if fields.is_empty() {
                    Ok(format!("Record<string, {additional}>"))
                } else {
                    Ok(format!("{object} & Record<string, {additional}>"))
                }
            }
            Some(Value::Bool(true)) if fields.is_empty() => {
                Ok("Record<string, unknown>".to_owned())
            }
            _ => Ok(object),
        }
    }
}

fn literal_type(value: &Value) -> Result<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).map_err(Error::Serialize)
        }
        _ => Err(Error::InvalidDescriptor(
            "operation JSON Schema contains a non-scalar literal".to_owned(),
        )),
    }
}

fn render_literals(values: &[Value]) -> Result<String> {
    let mut rendered = Vec::with_capacity(values.len());
    for value in values {
        rendered.push(literal_type(value)?);
    }
    Ok(join_types(rendered, " | "))
}

fn join_types(mut values: Vec<String>, separator: &str) -> String {
    values.sort();
    values.dedup();
    if values.is_empty() {
        "never".to_owned()
    } else if values.len() == 1 {
        values.pop().unwrap_or_else(|| "never".to_owned())
    } else {
        values.join(separator)
    }
}
