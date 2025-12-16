//! Multiple callbacks in one workflow example.
//!
//! Demonstrates:
//! - Two sequential `ctx.wait_for_callback()` operations, separated by other durable ops.
//! - Durable waits between invocations to make the state machine visible in history.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `{ "firstCallback": "...", "secondCallback": "...", ... }`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde_json::json;
use tracing_subscriber::EnvFilter;

pub async fn handler(
    _event: serde_json::Value,
    ctx: DurableContextHandle,
) -> DurableResult<serde_json::Value> {
    ctx.wait(Some("wait-invocation-1"), Duration::seconds(1))
        .await?;

    let callback_result_1: String = ctx
        .wait_for_callback(
            Some("first-callback"),
            |callback_id, step_ctx| async move {
                step_ctx.info(&format!("First callback submitted with ID: {callback_id}"));
                Ok(())
            },
            None,
        )
        .await?;

    let step_result: serde_json::Value = ctx
        .step(
            Some("process-callback-data"),
            |_step_ctx| async move { Ok(json!({ "processed": true, "step": 1 })) },
            None,
        )
        .await?;

    ctx.wait(Some("wait-invocation-2"), Duration::seconds(1))
        .await?;

    let callback_result_2: String = ctx
        .wait_for_callback(
            Some("second-callback"),
            |callback_id, step_ctx| async move {
                step_ctx.info(&format!("Second callback submitted with ID: {callback_id}"));
                Ok(())
            },
            None,
        )
        .await?;

    Ok(json!({
        "firstCallback": callback_result_1,
        "secondCallback": callback_result_2,
        "stepResult": step_result,
        "invocationCount": "multiple",
    }))
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
