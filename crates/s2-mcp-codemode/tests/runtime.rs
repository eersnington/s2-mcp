use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use s2_mcp_codemode::{CodeMode, Error, FunctionDescriptor, Invoker, Limits};
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

#[tokio::test]
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

#[tokio::test]
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
