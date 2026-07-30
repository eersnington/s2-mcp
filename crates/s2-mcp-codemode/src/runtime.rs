use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use deno_ast::{EmitOptions, SourceMapOption, TranspileModuleOptions, TranspileOptions};
use deno_core::{
    JsRuntime, ModuleCodeString, OpState, PollEventLoopOptions, RuntimeOptions,
    error::{CoreError, CoreErrorKind},
    op2, v8,
};
use deno_error::JsErrorBox;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Semaphore;

use crate::{CallOutcome, CallTrace, Error as PublicError, Invoker, Limits, validate_json_depth};

const RUNTIME_SOURCE: &str = include_str!("runtime.js");
const MODULE_SPECIFIER: &str = "file:///execute.js";

pub(crate) fn transpile(parsed: deno_ast::ParsedSource) -> Result<String, String> {
    let emitted = parsed
        .transpile(
            &TranspileOptions::default(),
            &TranspileModuleOptions::default(),
            &EmitOptions {
                source_map: SourceMapOption::None,
                inline_sources: false,
                ..Default::default()
            },
        )
        .map_err(|error| format!("TypeScript transpilation failed: {error}"))?
        .into_source();
    Ok(format!("{}\nexport default await run();", emitted.text))
}

#[derive(Clone)]
pub(crate) struct RuntimeInvoker {
    invoker: Invoker,
    public_names: Arc<BTreeMap<String, String>>,
    calls: Arc<AtomicUsize>,
    concurrency: Arc<Semaphore>,
    trace: Arc<Mutex<Trace>>,
    limits: Limits,
}

impl RuntimeInvoker {
    pub(crate) fn new(
        invoker: Invoker,
        public_names: BTreeMap<String, String>,
        limits: Limits,
    ) -> Self {
        Self {
            invoker,
            public_names: Arc::new(public_names),
            calls: Arc::new(AtomicUsize::new(0)),
            concurrency: Arc::new(Semaphore::new(limits.max_concurrent_calls)),
            trace: Arc::new(Mutex::new(Trace::default())),
            limits,
        }
    }

    fn begin(&self) -> Result<Option<u64>, RuntimeError> {
        let mut trace = self.trace.lock().map_err(|_| RuntimeError::Trace)?;
        if trace.next_sequence >= self.limits.max_callback_trace_calls as u64 {
            trace.truncated = true;
            return Ok(None);
        }
        let sequence = trace.next_sequence;
        trace.next_sequence = trace
            .next_sequence
            .checked_add(1)
            .ok_or(RuntimeError::Trace)?;
        Ok(Some(sequence))
    }

    fn finish(
        &self,
        sequence: Option<u64>,
        name: String,
        duration: Duration,
        outcome: CallOutcome,
    ) -> Result<(), RuntimeError> {
        let Some(sequence) = sequence else {
            return Ok(());
        };
        let mut trace = self.trace.lock().map_err(|_| RuntimeError::Trace)?;
        trace.calls.push(SequencedCall {
            sequence,
            call: CallTrace {
                name,
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                outcome,
            },
        });
        Ok(())
    }

    fn captured_calls(&self) -> Result<(Vec<CallTrace>, bool), RuntimeError> {
        let trace = self.trace.lock().map_err(|_| RuntimeError::Trace)?;
        let mut calls = trace.calls.clone();
        calls.sort_by_key(|call| call.sequence);
        Ok((
            calls.into_iter().map(|call| call.call).collect(),
            trace.truncated,
        ))
    }
}

#[derive(Default)]
struct Trace {
    next_sequence: u64,
    calls: Vec<SequencedCall>,
    truncated: bool,
}

#[derive(Clone)]
struct SequencedCall {
    sequence: u64,
    call: CallTrace,
}

#[op2]
#[serde]
fn op_codemode_extract_arguments<'s>(
    state: Rc<RefCell<OpState>>,
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<serde_json::Value, JsErrorBox> {
    let maximum_depth = state
        .borrow()
        .borrow::<RuntimeInvoker>()
        .limits
        .max_json_depth;
    extract_json(scope, value, maximum_depth).map_err(extract_error)
}

#[op2(async)]
#[serde]
async fn op_codemode_invoke(
    state: Rc<RefCell<OpState>>,
    #[string] operation: String,
    #[serde] arguments: Option<serde_json::Value>,
) -> Result<serde_json::Value, JsErrorBox> {
    let runtime_invoker = {
        let state = state.borrow();
        state.borrow::<RuntimeInvoker>().clone()
    };
    let Some(public_name) = runtime_invoker.public_names.get(&operation).cloned() else {
        return Err(JsErrorBox::generic("host operation was not found"));
    };
    let sequence = runtime_invoker.begin().map_err(runtime_error)?;
    let started_at = Instant::now();
    let call_number = runtime_invoker
        .calls
        .fetch_add(1, Ordering::SeqCst)
        .saturating_add(1);
    if call_number > runtime_invoker.limits.max_calls {
        runtime_invoker
            .finish(
                sequence,
                public_name,
                started_at.elapsed(),
                CallOutcome::Error,
            )
            .map_err(runtime_error)?;
        return Err(JsErrorBox::generic(format!(
            "execution exceeded the {} host operation limit",
            runtime_invoker.limits.max_calls
        )));
    }
    if let Some(arguments) = &arguments
        && let Err(error) = validate_json_depth(arguments, runtime_invoker.limits.max_json_depth)
    {
        runtime_invoker
            .finish(
                sequence,
                public_name,
                started_at.elapsed(),
                CallOutcome::Error,
            )
            .map_err(runtime_error)?;
        return Err(public_error(error));
    }
    let permit = runtime_invoker
        .concurrency
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| JsErrorBox::generic("host operation concurrency limiter closed"))?;
    let result = (runtime_invoker.invoker.inner)(operation, arguments).await;
    drop(permit);
    if let Ok(value) = &result
        && let Err(error) = validate_json_depth(value, runtime_invoker.limits.max_json_depth)
    {
        runtime_invoker
            .finish(
                sequence,
                public_name,
                started_at.elapsed(),
                CallOutcome::Error,
            )
            .map_err(runtime_error)?;
        return Err(public_error(error));
    }
    let outcome = if result.is_ok() {
        CallOutcome::Success
    } else {
        CallOutcome::Error
    };
    runtime_invoker
        .finish(sequence, public_name, started_at.elapsed(), outcome)
        .map_err(runtime_error)?;
    result.map_err(|error| JsErrorBox::generic(error.message().to_owned()))
}

fn extract_error(error: JsonExtractError) -> JsErrorBox {
    match error {
        JsonExtractError::Depth { maximum } => {
            public_error(PublicError::JsonDepthExceeded { maximum })
        }
        JsonExtractError::NotSerializable => JsErrorBox::generic("value is not JSON-serializable"),
    }
}

fn public_error(error: PublicError) -> JsErrorBox {
    JsErrorBox::generic(error.to_string())
}

fn runtime_error(_error: RuntimeError) -> JsErrorBox {
    JsErrorBox::generic("host operation bookkeeping failed")
}

deno_core::extension!(
    codemode_runtime,
    ops = [op_codemode_extract_arguments, op_codemode_invoke],
    options = { invoker: RuntimeInvoker },
    state = |state, options| state.put(options.invoker),
);

pub(crate) struct RuntimeExecution {
    pub(crate) success: bool,
    pub(crate) runtime_error: Option<String>,
    pub(crate) output: Option<Value>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) calls: Vec<CallTrace>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Error)]
pub(crate) enum RuntimeError {
    #[error("JavaScript runtime initialization failed: {0}")]
    Initialization(String),
    #[error("JavaScript runtime bootstrap failed: {0}")]
    Bootstrap(String),
    #[error("JavaScript runtime capture failed: {0}")]
    Capture(String),
    #[error("JSON value exceeds the maximum depth of {maximum}")]
    JsonDepthExceeded { maximum: usize },
    #[error("callback trace is unavailable")]
    Trace,
    #[error("JavaScript execution timed out")]
    Timeout,
}

impl RuntimeError {
    pub(crate) const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    pub(crate) const fn json_depth_maximum(&self) -> Option<usize> {
        match self {
            Self::JsonDepthExceeded { maximum } => Some(*maximum),
            _ => None,
        }
    }
}

struct ExecutionWatchdog {
    cancel: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    timed_out: Arc<AtomicBool>,
}

impl ExecutionWatchdog {
    fn start(handle: v8::IsolateHandle, timeout: Duration) -> Self {
        let (cancel, cancellation) = mpsc::channel();
        let timed_out = Arc::new(AtomicBool::new(false));
        let watchdog_timed_out = timed_out.clone();
        let thread = thread::spawn(move || {
            if cancellation.recv_timeout(timeout).is_err() {
                watchdog_timed_out.store(true, Ordering::SeqCst);
                handle.terminate_execution();
            }
        });
        Self {
            cancel,
            thread: Some(thread),
            timed_out,
        }
    }

    fn check(&self) -> Result<(), RuntimeError> {
        if self.timed_out.load(Ordering::SeqCst) {
            Err(RuntimeError::Timeout)
        } else {
            Ok(())
        }
    }
}

impl Drop for ExecutionWatchdog {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) async fn execute(
    source: String,
    invoker: RuntimeInvoker,
    limits: Limits,
) -> Result<RuntimeExecution, RuntimeError> {
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        create_params: Some(v8::Isolate::create_params().heap_limits(0, limits.max_heap_bytes)),
        module_loader: Some(Rc::new(deno_core::NoopModuleLoader)),
        extensions: vec![codemode_runtime::init(invoker.clone())],
        ..Default::default()
    })
    .map_err(|error| RuntimeError::Initialization(core_error_message(&error)))?;
    let watchdog = ExecutionWatchdog::start(
        runtime.v8_isolate().thread_safe_handle(),
        limits.execution_timeout,
    );
    runtime
        .execute_script("<runtime_bootstrap>", RUNTIME_SOURCE)
        .map_err(|error| RuntimeError::Bootstrap(js_error_message(&error)))?;
    let namespace = invoker
        .public_names
        .iter()
        .map(|(operation, public_name)| {
            let local_name = public_name.strip_prefix("S2.").unwrap_or(public_name);
            (local_name, operation)
        })
        .collect::<BTreeMap<_, _>>();
    let namespace = serde_json::to_string(&namespace)
        .map_err(|error| RuntimeError::Bootstrap(error.to_string()))?;
    runtime
        .execute_script(
            "<runtime_namespace>",
            format!(
                "globalThis.__codeModeInstallNamespace({namespace}, {}); delete globalThis.__codeModeInstallNamespace;",
                limits.max_console_bytes
            ),
        )
        .map_err(|error| RuntimeError::Bootstrap(js_error_message(&error)))?;

    let specifier = deno_core::resolve_url(MODULE_SPECIFIER)
        .map_err(|error| RuntimeError::Initialization(error.to_string()))?;
    let module_id = match runtime
        .load_side_es_module_from_code(&specifier, ModuleCodeString::from(source))
        .await
    {
        Ok(module_id) => module_id,
        Err(error) => {
            return finish(
                &mut runtime,
                &invoker,
                None,
                Some(core_error_message(&error)),
                limits,
            );
        }
    };
    let evaluation = runtime.mod_evaluate(module_id);
    let event_loop_error = runtime
        .run_event_loop(PollEventLoopOptions {
            wait_for_inspector: false,
            pump_v8_message_loop: true,
        })
        .await
        .err();
    let evaluation_error = evaluation.await.err();
    let runtime_error = evaluation_error
        .as_ref()
        .or(event_loop_error.as_ref())
        .map(core_error_message);
    let output = if runtime_error.is_none() {
        extract_output(&mut runtime, module_id, limits.max_json_depth)?
    } else {
        None
    };
    watchdog.check()?;
    finish(&mut runtime, &invoker, output, runtime_error, limits)
}

fn finish(
    runtime: &mut JsRuntime,
    invoker: &RuntimeInvoker,
    output: Option<Value>,
    runtime_error: Option<String>,
    limits: Limits,
) -> Result<RuntimeExecution, RuntimeError> {
    let (mut stdout, mut stderr, capture_truncated) = capture_console(runtime)?;
    let defensive_truncation = bound_streams(&mut stdout, &mut stderr, limits.max_console_bytes);
    let (calls, trace_truncated) = invoker.captured_calls()?;
    Ok(RuntimeExecution {
        success: runtime_error.is_none(),
        runtime_error,
        output,
        stdout,
        stderr,
        calls,
        truncated: capture_truncated || defensive_truncation || trace_truncated,
    })
}

fn extract_output(
    runtime: &mut JsRuntime,
    module_id: usize,
    maximum_depth: usize,
) -> Result<Option<Value>, RuntimeError> {
    let namespace = runtime
        .get_module_namespace(module_id)
        .map_err(|error| RuntimeError::Capture(core_error_message(&error)))?;
    deno_core::scope!(scope, runtime);
    let namespace = v8::Local::new(scope, namespace);
    let Some(default_key) = v8::String::new(scope, "default") else {
        return Ok(None);
    };
    let Some(default_value) = namespace.get(scope, default_key.into()) else {
        return Ok(None);
    };
    if default_value.is_undefined() {
        return Ok(None);
    }
    match extract_json(scope, default_value, maximum_depth) {
        Ok(value) => Ok(Some(value)),
        Err(JsonExtractError::Depth { maximum }) => {
            Err(RuntimeError::JsonDepthExceeded { maximum })
        }
        Err(JsonExtractError::NotSerializable) => Ok(None),
    }
}

#[derive(Debug)]
enum JsonExtractError {
    Depth { maximum: usize },
    NotSerializable,
}

fn extract_json<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    maximum_depth: usize,
) -> Result<Value, JsonExtractError> {
    inspect_json_depth(scope, value, 0, maximum_depth, &mut Vec::new())?;
    deno_core::serde_v8::from_v8(scope, value).map_err(|_| JsonExtractError::NotSerializable)
}

fn inspect_json_depth<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    parent_depth: usize,
    maximum_depth: usize,
    ancestors: &mut Vec<v8::Local<'s, v8::Object>>,
) -> Result<(), JsonExtractError> {
    if !value.is_array() && !value.is_object() {
        return Ok(());
    }
    if value.is_proxy() {
        return Err(JsonExtractError::NotSerializable);
    }
    let depth = parent_depth.saturating_add(1);
    if depth > maximum_depth {
        return Err(JsonExtractError::Depth {
            maximum: maximum_depth,
        });
    }
    let object = value
        .to_object(scope)
        .ok_or(JsonExtractError::NotSerializable)?;
    if ancestors
        .iter()
        .any(|ancestor| ancestor.strict_equals(object.into()))
    {
        return Err(JsonExtractError::NotSerializable);
    }
    ancestors.push(object);
    let result = if value.is_array() {
        inspect_array_items(scope, object, depth, maximum_depth, ancestors)
    } else {
        inspect_object_properties(scope, object, depth, maximum_depth, ancestors)
    };
    ancestors.pop();
    result
}

fn inspect_array_items<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    depth: usize,
    maximum_depth: usize,
    ancestors: &mut Vec<v8::Local<'s, v8::Object>>,
) -> Result<(), JsonExtractError> {
    let array =
        v8::Local::<v8::Array>::try_from(object).map_err(|_| JsonExtractError::NotSerializable)?;
    let keys = own_property_names(scope, object)?;
    let value_key = v8::String::new(scope, "value").ok_or(JsonExtractError::NotSerializable)?;
    let mut item_count = 0;
    for index in 0..keys.length() {
        let key = keys
            .get_index(scope, index)
            .ok_or(JsonExtractError::NotSerializable)?;
        let key_text = key.to_rust_string_lossy(scope);
        let Ok(item_index) = key_text.parse::<u32>() else {
            continue;
        };
        if item_index.to_string() != key_text || item_index >= array.length() {
            continue;
        }
        let name =
            v8::Local::<v8::Name>::try_from(key).map_err(|_| JsonExtractError::NotSerializable)?;
        let child = data_property_value(scope, object, name, value_key)?;
        inspect_json_depth(scope, child, depth, maximum_depth, ancestors)?;
        item_count += 1;
    }
    if item_count != array.length() {
        return Err(JsonExtractError::NotSerializable);
    }
    Ok(())
}

fn inspect_object_properties<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    depth: usize,
    maximum_depth: usize,
    ancestors: &mut Vec<v8::Local<'s, v8::Object>>,
) -> Result<(), JsonExtractError> {
    let keys = own_property_names(scope, object)?;
    let value_key = v8::String::new(scope, "value").ok_or(JsonExtractError::NotSerializable)?;
    for index in 0..keys.length() {
        let key = keys
            .get_index(scope, index)
            .ok_or(JsonExtractError::NotSerializable)?;
        let name =
            v8::Local::<v8::Name>::try_from(key).map_err(|_| JsonExtractError::NotSerializable)?;
        let child = data_property_value(scope, object, name, value_key)?;
        inspect_json_depth(scope, child, depth, maximum_depth, ancestors)?;
    }
    Ok(())
}

fn own_property_names<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
) -> Result<v8::Local<'s, v8::Array>, JsonExtractError> {
    object
        .get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                mode: v8::KeyCollectionMode::OwnOnly,
                key_conversion: v8::KeyConversionMode::ConvertToString,
                ..Default::default()
            },
        )
        .ok_or(JsonExtractError::NotSerializable)
}

fn data_property_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    name: v8::Local<'s, v8::Name>,
    value_key: v8::Local<'s, v8::String>,
) -> Result<v8::Local<'s, v8::Value>, JsonExtractError> {
    let descriptor = object
        .get_own_property_descriptor(scope, name)
        .and_then(|descriptor| descriptor.to_object(scope))
        .ok_or(JsonExtractError::NotSerializable)?;
    if descriptor.has_own_property(scope, value_key.into()) != Some(true) {
        return Err(JsonExtractError::NotSerializable);
    }
    descriptor
        .get(scope, value_key.into())
        .ok_or(JsonExtractError::NotSerializable)
}

#[derive(Deserialize)]
struct ConsoleCapture {
    stdout: Vec<String>,
    stderr: Vec<String>,
    truncated: bool,
}

fn capture_console(runtime: &mut JsRuntime) -> Result<(String, String, bool), RuntimeError> {
    let capture = runtime
        .execute_script("<runtime_capture>", "globalThis.__codeModeRuntimeCapture()")
        .map_err(|error| RuntimeError::Capture(js_error_message(&error)))?;
    deno_core::scope!(scope, runtime);
    let capture = v8::Local::new(scope, capture);
    let capture = deno_core::serde_v8::from_v8::<ConsoleCapture>(scope, capture)
        .map_err(|error| RuntimeError::Capture(error.to_string()))?;
    Ok((
        capture.stdout.join("\n"),
        capture.stderr.join("\n"),
        capture.truncated,
    ))
}

fn core_error_message(error: &CoreError) -> String {
    core_error_kind_message(error.0.as_ref())
}

fn js_error_message(error: &deno_core::error::JsError) -> String {
    error.exception_message.trim().to_owned()
}

fn core_error_kind_message(error: &CoreErrorKind) -> String {
    match error {
        CoreErrorKind::Js(error) => error.exception_message.trim().to_owned(),
        CoreErrorKind::CouldNotExecute { error, .. } => core_error_kind_message(error),
        _ => error.to_string(),
    }
}

fn bound_streams(stdout: &mut String, stderr: &mut String, maximum: usize) -> bool {
    if stdout.len().saturating_add(stderr.len()) <= maximum {
        return false;
    }
    let stdout_truncated = truncate_utf8(stdout, maximum);
    let remaining = maximum.saturating_sub(stdout.len());
    let stderr_truncated = truncate_utf8(stderr, remaining);
    stdout_truncated || stderr_truncated
}

fn truncate_utf8(value: &mut String, maximum: usize) -> bool {
    if value.len() <= maximum {
        return false;
    }
    let mut boundary = maximum;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    true
}
