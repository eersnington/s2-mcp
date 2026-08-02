use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use s2_mcp_codemode::{CodeMode, Error, FunctionDescriptor, InvokeError, Invoker, Limits};
use serde_json::json;

fn code_mode() -> Result<CodeMode, Error> {
    CodeMode::new(vec![FunctionDescriptor {
        operation: "test".to_owned(),
        name: "S2.test".to_owned(),
        description: "Test operation.".to_owned(),
        signature: "function test(input: unknown): Promise<unknown>;".to_owned(),
        input_schema: json!({}),
    }])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_argument_does_not_call_invoker() -> Result<(), Error> {
    let calls = Arc::new(AtomicUsize::new(0));
    let invoker_calls = calls.clone();
    let invoker = Invoker::new(move |_, _| {
        invoker_calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(json!(null)) }
    });
    let limits = Limits {
        max_json_depth: 2,
        ..Limits::default()
    };

    let _ = code_mode()?
        .execute(
            "async function run() { return await S2.test({ a: { b: { c: 1 } } }); }",
            invoker,
            limits,
        )
        .await?;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_output_returns_json_depth_exceeded() -> Result<(), Error> {
    let limits = Limits {
        max_json_depth: 2,
        ..Limits::default()
    };
    let result = code_mode()?
        .execute(
            "async function run() { return { a: { b: { c: 1 } } }; }",
            Invoker::new(|_, _| async { Ok(json!(null)) }),
            limits,
        )
        .await;

    assert!(matches!(
        result,
        Err(Error::JsonDepthExceeded { maximum: 2 })
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronous_javascript_is_terminated_by_the_tokio_watchdog() -> Result<(), Error> {
    let limits = Limits {
        execution_timeout: Duration::from_secs(1),
        ..Limits::default()
    };
    let result = code_mode()?
        .execute(
            "async function run() { while (true) {} }",
            Invoker::new(|_, _| async { Ok(json!(null)) }),
            limits,
        )
        .await;

    assert!(matches!(
        result,
        Err(Error::ExecutionTimeout { seconds: 1 })
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_invoke_error_is_structured_in_javascript() -> Result<(), Error> {
    let output = code_mode()?
        .execute(
            r#"async function run() {
                try { await S2.test({}); }
                catch (error) { return { name: error.name, code: error.code, remediation: error.remediation }; }
            }"#,
            Invoker::new(|_, _| async {
                Err(InvokeError::public_with_details(
                    "test_failed",
                    "test failed",
                    Some("retry the test".to_owned()),
                ))
            }),
            Limits::default(),
        )
        .await?;

    assert_eq!(
        output.output,
        Some(json!({
            "name": "S2InvokeError",
            "code": "test_failed",
            "remediation": "retry the test"
        }))
    );
    Ok(())
}
