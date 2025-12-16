//! Durable invoke caller example.
//!
//! Demonstrates:
//! - `ctx.invoke()` to call another Lambda function as a durable operation.
//! - Passing a typed payload to the target and using the typed result in a follow-up step.
//!
//! Event (JSON):
//! - `{ "value": 21 }`
//!
//! Result (JSON):
//! - `{ "input": 21, "target_doubled": 42, "plus_one": 43 }`
//!
//! Deployment note: the invoke target is configured via the `INVOKE_TARGET_FUNCTION`
//! environment variable (set in `examples/template.yaml`).

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeCallerEvent {
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeCallerResponse {
    pub input: i32,
    pub target_doubled: i32,
    pub plus_one: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvokeTargetEvent {
    value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvokeTargetResponse {
    doubled: i32,
}

pub async fn handler(
    event: InvokeCallerEvent,
    ctx: DurableContextHandle,
) -> DurableResult<InvokeCallerResponse> {
    let mut target = std::env::var("INVOKE_TARGET_FUNCTION").map_err(|_| {
        DurableError::InvalidConfiguration {
            message: "Missing INVOKE_TARGET_FUNCTION".to_string(),
        }
    })?;

    // Durable invoke rejects unqualified ARNs; normalize `...:function:NAME` to `...:function:NAME:$LATEST`.
    if target.starts_with("arn:") {
        if let Some(after) = target.split(":function:").nth(1) {
            if !after.contains(':') {
                target.push_str(":$LATEST");
            }
        }
    }

    // Treat the invoke itself as a durable operation (checkpointed + replay-safe).
    let target_result: InvokeTargetResponse = ctx
        .invoke(
            Some("invoke-target"),
            &target,
            Some(InvokeTargetEvent { value: event.value }),
        )
        .await?;

    // Demonstrate additional work after the invoke completes.
    let plus_one = ctx
        .step(
            Some("plus-one"),
            move |_step_ctx| async move { Ok(target_result.doubled + 1) },
            None,
        )
        .await?;

    Ok(InvokeCallerResponse {
        input: event.value,
        target_doubled: target_result.doubled,
        plus_one,
    })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
