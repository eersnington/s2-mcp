mod runtime;
mod schema;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    future::Future,
    io::{self, Write},
    mem,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use deno_ast::{
    MediaType, ModuleSpecifier, ParseParams, parse_module,
    swc::{
        ast::{CallExpr, Callee, ModuleDecl},
        ecma_visit::{Visit, VisitWith, noop_visit_type},
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::runtime::{RuntimeExecution, RuntimeInvoker};

const DEFAULT_SEARCH_RESULTS: usize = 8;
const MAX_SEARCH_RESULTS: usize = 25;
const MAX_SEARCH_OUTPUT_BYTES: usize = 256 * 1024;
const SEARCH_CURSOR_PREFIX: &str = "v1:";
const OUTPUT_LIMIT_MESSAGE: &str = "Returned value exceeded the configured output limit.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionDescriptor {
    pub operation: String,
    pub name: String,
    pub description: String,
    pub signature: String,
    #[serde(default = "empty_schema")]
    pub input_schema: Value,
}

impl FunctionDescriptor {
    pub fn from_schemas(
        operation: &str,
        description: &str,
        input_schema: Value,
        output_schema: Value,
    ) -> Result<Self, Error> {
        schema::generate_function(operation, description, input_schema, output_schema)
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    #[serde(default)]
    pub query: String,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SearchMatch {
    pub name: String,
    pub description: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SearchOutput {
    pub matches: Vec<SearchMatch>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteInput {
    pub code: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_source_bytes: usize,
    pub max_output_bytes: usize,
    pub max_json_depth: usize,
    pub max_calls: usize,
    pub max_concurrent_calls: usize,
    pub max_console_bytes: usize,
    pub max_callback_trace_calls: usize,
    pub max_heap_bytes: usize,
    pub execution_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_bytes: 64 * 1024,
            max_output_bytes: 256 * 1024,
            max_json_depth: 32,
            max_calls: 32,
            max_concurrent_calls: 8,
            max_console_bytes: 256 * 1024,
            max_callback_trace_calls: 33,
            max_heap_bytes: 128 * 1024 * 1024,
            execution_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeDiagnostic {
    pub code: String,
    pub message: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeError {
    Public(InvokeDiagnostic),
    Private,
}

impl InvokeError {
    pub fn public(message: impl Into<String>) -> Self {
        Self::public_with_details("operation_failed", message, None)
    }

    pub fn public_with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self::Public(InvokeDiagnostic {
            code: code.into(),
            message: message.into(),
            remediation,
        })
    }

    pub const fn private() -> Self {
        Self::Private
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Public(diagnostic) => &diagnostic.message,
            Self::Private => "host operation failed",
        }
    }
}

pub type InvokeFuture = Pin<Box<dyn Future<Output = Result<Value, InvokeError>> + Send + 'static>>;

#[derive(Clone)]
pub struct Invoker {
    pub(crate) inner: Arc<dyn Fn(String, Option<Value>) -> InvokeFuture + Send + Sync>,
}

impl Invoker {
    pub fn new<Invoke, InvokeResult>(invoke: Invoke) -> Self
    where
        Invoke: Fn(String, Option<Value>) -> InvokeResult + Send + Sync + 'static,
        InvokeResult: Future<Output = Result<Value, InvokeError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |operation, arguments| Box::pin(invoke(operation, arguments))),
        }
    }
}

impl fmt::Debug for Invoker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Invoker").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Transpile,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteDiagnostic {
    pub phase: DiagnosticPhase,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallTrace {
    pub name: String,
    pub duration_ms: u64,
    pub outcome: CallOutcome,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteOutput {
    pub success: bool,
    pub output: Option<Value>,
    pub diagnostics: Vec<ExecuteDiagnostic>,
    pub stdout: String,
    pub stderr: String,
    pub tool_calls: Vec<CallTrace>,
    pub truncated: bool,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid Code Mode descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("invalid search arguments: {0}")]
    InvalidSearch(String),
    #[error("code exceeds the {maximum} byte source limit")]
    SourceTooLarge { maximum: usize },
    #[error("module imports are not allowed in Code Mode")]
    ImportsNotAllowed,
    #[error("TypeScript could not be parsed: {0}")]
    Parse(String),
    #[error("JSON value exceeds the maximum depth of {maximum}")]
    JsonDepthExceeded { maximum: usize },
    #[error("code execution timed out after {seconds} seconds")]
    ExecutionTimeout { seconds: u64 },
    #[error("Code Mode runtime failed: {0}")]
    Runtime(String),
    #[error("failed to serialize Code Mode output: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct CodeMode {
    functions: Arc<[IndexedFunction]>,
}

impl CodeMode {
    pub fn new(descriptors: Vec<FunctionDescriptor>) -> Result<Self, Error> {
        let mut operations = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut functions = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            validate_descriptor(&descriptor)?;
            if !operations.insert(descriptor.operation.clone()) {
                return Err(Error::InvalidDescriptor(format!(
                    "operation `{}` is duplicated",
                    descriptor.operation
                )));
            }
            if !names.insert(descriptor.name.clone()) {
                return Err(Error::InvalidDescriptor(format!(
                    "function name `{}` is duplicated",
                    descriptor.name
                )));
            }
            functions.push(index_function(descriptor));
        }
        functions.sort_by(|left, right| left.descriptor.name.cmp(&right.descriptor.name));
        Ok(Self {
            functions: functions.into(),
        })
    }

    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = &FunctionDescriptor> {
        self.functions.iter().map(|function| &function.descriptor)
    }

    pub fn search(&self, input: SearchInput) -> Result<SearchOutput, Error> {
        let limit = input.limit.unwrap_or(DEFAULT_SEARCH_RESULTS);
        if limit == 0 || limit > MAX_SEARCH_RESULTS {
            return Err(Error::InvalidSearch(format!(
                "limit must be between 1 and {MAX_SEARCH_RESULTS}"
            )));
        }
        let query = input.query.trim();
        let query_terms = tokenize(query);
        let mut ranked = self
            .functions
            .iter()
            .filter_map(|function| {
                let rank = search_rank(function, query, &query_terms);
                (query.is_empty() || rank != SearchRank::default()).then_some((rank, function))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_rank, left), (right_rank, right)| {
            right_rank
                .cmp(left_rank)
                .then_with(|| left.descriptor.name.cmp(&right.descriptor.name))
        });
        let start = cursor_start(input.cursor.as_deref(), &ranked)?;
        let mut matches = Vec::with_capacity(limit.min(ranked.len().saturating_sub(start)));
        let mut end = start;
        while end < ranked.len() && matches.len() < limit {
            let function = ranked[end].1;
            matches.push(SearchMatch {
                name: function.descriptor.name.clone(),
                description: function.descriptor.description.clone(),
                signature: function.descriptor.signature.clone(),
            });
            end += 1;
            let prospective = SearchOutput {
                matches: matches.clone(),
                next_cursor: (end < ranked.len())
                    .then(|| matches.last().map(|entry| search_cursor(&entry.name)))
                    .flatten(),
            };
            if serialized_size(&prospective)? > MAX_SEARCH_OUTPUT_BYTES {
                matches.pop();
                end -= 1;
                if matches.is_empty() {
                    return Err(Error::InvalidSearch(
                        "one search match exceeds the 256 KiB output limit".to_owned(),
                    ));
                }
                break;
            }
        }
        let next_cursor = (end < ranked.len())
            .then(|| matches.last().map(|entry| search_cursor(&entry.name)))
            .flatten();
        Ok(SearchOutput {
            matches,
            next_cursor,
        })
    }

    pub async fn execute(
        &self,
        source: &str,
        invoker: Invoker,
        limits: Limits,
    ) -> Result<ExecuteOutput, Error> {
        validate_limits(limits)?;
        if source.len() > limits.max_source_bytes {
            return Err(Error::SourceTooLarge {
                maximum: limits.max_source_bytes,
            });
        }
        let parsed = parse_source(source)?;
        reject_module_imports(&parsed)?;
        let transpiled = match runtime::transpile(parsed) {
            Ok(source) => source,
            Err(message) => {
                return bound_output(transpile_failure(message), limits.max_output_bytes);
            }
        };
        let descriptors = self
            .functions
            .iter()
            .map(|function| {
                (
                    function.descriptor.operation.clone(),
                    function.descriptor.name.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let runtime_invoker = RuntimeInvoker::new(invoker, descriptors, limits);
        let execution = tokio::time::timeout(
            limits.execution_timeout,
            runtime::execute(transpiled, runtime_invoker, limits),
        )
        .await
        .map_err(|_| Error::ExecutionTimeout {
            seconds: limits.execution_timeout.as_secs(),
        })?
        .map_err(|error| {
            if error.is_timeout() {
                Error::ExecutionTimeout {
                    seconds: limits.execution_timeout.as_secs(),
                }
            } else if let Some(maximum) = error.json_depth_maximum() {
                Error::JsonDepthExceeded { maximum }
            } else {
                Error::Runtime(error.to_string())
            }
        })?;
        map_execution(execution, limits)
    }
}

#[derive(Debug, Clone)]
struct IndexedFunction {
    descriptor: FunctionDescriptor,
    name_segments: BTreeSet<String>,
    description_terms: BTreeSet<String>,
    input_property_name_terms: BTreeSet<String>,
    input_property_description_terms: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct SearchRank {
    exact_full_name: bool,
    name_segments: usize,
    description_terms: usize,
    input_property_name_terms: usize,
    input_property_description_terms: usize,
}

fn validate_descriptor(descriptor: &FunctionDescriptor) -> Result<(), Error> {
    if descriptor.operation.is_empty() {
        return Err(Error::InvalidDescriptor(
            "operation name is empty".to_owned(),
        ));
    }
    let Some(local_name) = descriptor.name.strip_prefix("S2.") else {
        return Err(Error::InvalidDescriptor(format!(
            "function `{}` is not in the S2 namespace",
            descriptor.name
        )));
    };
    let mut characters = local_name.chars();
    let valid_first = characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_ascii_alphabetic()
    });
    if !valid_first
        || !characters.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    {
        return Err(Error::InvalidDescriptor(format!(
            "function name `{}` is not a supported TypeScript identifier",
            descriptor.name
        )));
    }
    Ok(())
}

fn validate_limits(limits: Limits) -> Result<(), Error> {
    if limits.max_source_bytes == 0
        || limits.max_output_bytes == 0
        || limits.max_json_depth == 0
        || limits.max_calls == 0
        || limits.max_concurrent_calls == 0
        || limits.max_console_bytes == 0
        || limits.max_callback_trace_calls == 0
        || limits.max_heap_bytes == 0
        || limits.execution_timeout.is_zero()
    {
        return Err(Error::Runtime(
            "all Code Mode limits must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn empty_schema() -> Value {
    Value::Object(Default::default())
}

fn index_function(descriptor: FunctionDescriptor) -> IndexedFunction {
    let mut input_property_name_terms = BTreeSet::new();
    let mut input_property_description_terms = BTreeSet::new();
    collect_input_property_terms(
        &descriptor.input_schema,
        &mut input_property_name_terms,
        &mut input_property_description_terms,
    );
    IndexedFunction {
        name_segments: tokenize(&descriptor.name),
        description_terms: tokenize(&descriptor.description),
        input_property_name_terms,
        input_property_description_terms,
        descriptor,
    }
}

fn collect_input_property_terms(
    value: &Value,
    property_names: &mut BTreeSet<String>,
    property_descriptions: &mut BTreeSet<String>,
) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(properties)) = object.get("properties") {
                for (name, schema) in properties {
                    property_names.extend(tokenize(name));
                    if let Value::Object(schema) = schema
                        && let Some(Value::String(description)) = schema.get("description")
                    {
                        property_descriptions.extend(tokenize(description));
                    }
                }
            }
            for child in object.values() {
                collect_input_property_terms(child, property_names, property_descriptions);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_input_property_terms(child, property_names, property_descriptions);
            }
        }
        _ => {}
    }
}

fn search_rank(
    function: &IndexedFunction,
    query: &str,
    query_terms: &BTreeSet<String>,
) -> SearchRank {
    SearchRank {
        exact_full_name: query.eq_ignore_ascii_case(&function.descriptor.name),
        name_segments: query_terms.intersection(&function.name_segments).count(),
        description_terms: query_terms
            .intersection(&function.description_terms)
            .count(),
        input_property_name_terms: query_terms
            .intersection(&function.input_property_name_terms)
            .count(),
        input_property_description_terms: query_terms
            .intersection(&function.input_property_description_terms)
            .count(),
    }
}

fn tokenize(value: &str) -> BTreeSet<String> {
    let characters = value.chars().collect::<Vec<_>>();
    let mut terms = BTreeSet::new();
    let mut current = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            push_term(&mut terms, &mut current);
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        let boundary = character.is_ascii_uppercase()
            && !current.is_empty()
            && (previous.is_some_and(|previous| {
                previous.is_ascii_lowercase() || previous.is_ascii_digit()
            }) || (previous.is_some_and(|previous| previous.is_ascii_uppercase())
                && next.is_some_and(|next| next.is_ascii_lowercase())));
        if boundary {
            push_term(&mut terms, &mut current);
        }
        current.push(character.to_ascii_lowercase());
    }
    push_term(&mut terms, &mut current);
    terms
}

fn push_term(terms: &mut BTreeSet<String>, current: &mut String) {
    if !current.is_empty() {
        terms.insert(mem::take(current));
    }
}

fn cursor_start(
    cursor: Option<&str>,
    ranked: &[(SearchRank, &IndexedFunction)],
) -> Result<usize, Error> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(name) = cursor.strip_prefix(SEARCH_CURSOR_PREFIX) else {
        return Err(Error::InvalidSearch("search cursor is invalid".to_owned()));
    };
    let Some(position) = ranked
        .iter()
        .position(|(_, function)| function.descriptor.name == name)
    else {
        return Err(Error::InvalidSearch(
            "search cursor is invalid for this query".to_owned(),
        ));
    };
    Ok(position + 1)
}

fn search_cursor(name: &str) -> String {
    format!("{SEARCH_CURSOR_PREFIX}{name}")
}

#[derive(Default)]
struct ImportDetector {
    found: bool,
}

impl Visit for ImportDetector {
    noop_visit_type!();

    fn visit_module_decl(&mut self, _declaration: &ModuleDecl) {
        self.found = true;
    }

    fn visit_call_expr(&mut self, expression: &CallExpr) {
        if matches!(expression.callee, Callee::Import(_)) {
            self.found = true;
        }
        expression.visit_children_with(self);
    }
}

fn parse_source(source: &str) -> Result<deno_ast::ParsedSource, Error> {
    let specifier = ModuleSpecifier::parse("file:///execute.ts")
        .map_err(|error| Error::Parse(error.to_string()))?;
    parse_module(ParseParams {
        specifier,
        text: source.to_owned().into(),
        media_type: MediaType::TypeScript,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|error| Error::Parse(error.to_string()))
}

fn reject_module_imports(parsed: &deno_ast::ParsedSource) -> Result<(), Error> {
    let mut detector = ImportDetector::default();
    parsed.program_ref().visit_with(&mut detector);
    if detector.found {
        return Err(Error::ImportsNotAllowed);
    }
    Ok(())
}

fn map_execution(execution: RuntimeExecution, limits: Limits) -> Result<ExecuteOutput, Error> {
    if let Some(output) = &execution.output {
        validate_json_depth(output, limits.max_json_depth)?;
    }
    let mut diagnostics = Vec::new();
    if let Some(message) = execution.runtime_error {
        diagnostics.push(ExecuteDiagnostic {
            phase: DiagnosticPhase::Runtime,
            message: sanitize_runtime_text(&message),
        });
    } else if execution.output.is_none() {
        diagnostics.push(ExecuteDiagnostic {
            phase: DiagnosticPhase::Runtime,
            message: "run() did not return a JSON-serializable value".to_owned(),
        });
    }
    let output = ExecuteOutput {
        success: execution.success && execution.output.is_some(),
        output: execution.output,
        diagnostics,
        stdout: execution.stdout,
        stderr: execution.stderr,
        tool_calls: execution.calls,
        truncated: execution.truncated,
    };
    bound_output(output, limits.max_output_bytes)
}

fn transpile_failure(message: String) -> ExecuteOutput {
    ExecuteOutput {
        success: false,
        output: None,
        diagnostics: vec![ExecuteDiagnostic {
            phase: DiagnosticPhase::Transpile,
            message: sanitize_runtime_text(&message),
        }],
        stdout: String::new(),
        stderr: String::new(),
        tool_calls: Vec::new(),
        truncated: false,
    }
}

fn sanitize_runtime_text(value: &str) -> String {
    value
        .replace("file:///execute.js", "execute.ts")
        .replace("file:///execute.ts", "execute.ts")
        .trim()
        .to_owned()
}

pub fn validate_json_depth(value: &Value, maximum: usize) -> Result<(), Error> {
    fn visit(value: &Value, parent_depth: usize, maximum: usize) -> Result<(), Error> {
        let depth = parent_depth.saturating_add(1);
        match value {
            Value::Array(values) => {
                if depth > maximum {
                    return Err(Error::JsonDepthExceeded { maximum });
                }
                for child in values {
                    visit(child, depth, maximum)?;
                }
            }
            Value::Object(values) => {
                if depth > maximum {
                    return Err(Error::JsonDepthExceeded { maximum });
                }
                for child in values.values() {
                    visit(child, depth, maximum)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, 0, maximum)
}

fn bound_output(mut output: ExecuteOutput, maximum: usize) -> Result<ExecuteOutput, Error> {
    const MAX_DIAGNOSTICS: usize = 64;
    const MAX_DIAGNOSTIC_BYTES: usize = 8 * 1024;
    output.truncated |= output.diagnostics.len() > MAX_DIAGNOSTICS;
    output.diagnostics.truncate(MAX_DIAGNOSTICS);
    for diagnostic in &mut output.diagnostics {
        let original = diagnostic.message.len();
        truncate_json_string(&mut diagnostic.message, MAX_DIAGNOSTIC_BYTES);
        output.truncated |= diagnostic.message.len() < original;
    }
    if serialized_size(&output)? <= maximum {
        return Ok(output);
    }
    output.truncated = true;
    let stdout = mem::take(&mut output.stdout);
    let stderr = mem::take(&mut output.stderr);
    if serialized_size(&output)? > maximum && output.output.is_some() {
        output.success = false;
        output.output = None;
        output.stderr = OUTPUT_LIMIT_MESSAGE.to_owned();
    }
    while serialized_size(&output)? > maximum && !output.diagnostics.is_empty() {
        output.diagnostics.pop();
    }
    if serialized_size(&output)? > maximum {
        output.tool_calls.clear();
    }
    output.stdout = stdout;
    if output.stderr.is_empty() {
        output.stderr = stderr;
    } else if !stderr.is_empty() {
        output.stderr.push('\n');
        output.stderr.push_str(&stderr);
    }
    truncate_streams(&mut output, maximum)?;
    Ok(output)
}

fn truncate_streams(output: &mut ExecuteOutput, maximum: usize) -> Result<(), Error> {
    let mut stdout = mem::take(&mut output.stdout);
    let mut stderr = mem::take(&mut output.stderr);
    let available = maximum.saturating_sub(serialized_size(output)?);
    let stdout_size = serialized_size(&stdout)?.saturating_sub(2);
    let stderr_size = serialized_size(&stderr)?.saturating_sub(2);
    let mut stdout_budget = stdout_size.min(available / 2);
    let mut stderr_budget = stderr_size.min(available / 2);
    let mut remaining = available.saturating_sub(stdout_budget + stderr_budget);
    let extra = (stdout_size - stdout_budget).min(remaining);
    stdout_budget += extra;
    remaining -= extra;
    stderr_budget += (stderr_size - stderr_budget).min(remaining);
    truncate_json_string(&mut stdout, stdout_budget);
    truncate_json_string(&mut stderr, stderr_budget);
    output.stdout = stdout;
    output.stderr = stderr;
    if serialized_size(output)? > maximum {
        output.stdout.clear();
        output.stderr.clear();
    }
    if serialized_size(output)? > maximum {
        return Err(Error::Runtime(format!(
            "the configured {maximum} byte output limit is too small for an execution envelope"
        )));
    }
    Ok(())
}

fn truncate_json_string(value: &mut String, maximum: usize) {
    let mut size: usize = 0;
    let mut boundary = 0;
    for (index, character) in value.char_indices() {
        let character_size = match character {
            '"' | '\\' | '\u{08}' | '\t' | '\n' | '\u{0c}' | '\r' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => character.len_utf8(),
        };
        if size.saturating_add(character_size) > maximum {
            break;
        }
        size += character_size;
        boundary = index + character.len_utf8();
    }
    value.truncate(boundary);
}

fn serialized_size<T: Serialize + ?Sized>(value: &T) -> Result<usize, Error> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
