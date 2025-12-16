//! Durable invoke target example.
//!
//! Demonstrates:
//! - A simple durable function designed to be called via `ctx.invoke()` from another workflow.
//! - Performing work inside a checkpointed `ctx.step()`.
//!
//! Event (JSON):
//! - `{ "value": 21 }`
//!
//! Result (JSON):
//! - `{ "doubled": 42 }`
//!
//! Deployed via `examples/template.yaml` and used by `invoke_caller`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeTargetEvent {
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeTargetResponse {
    pub doubled: i32,
}

pub async fn handler(
    event: InvokeTargetEvent,
    ctx: DurableContextHandle,
) -> DurableResult<InvokeTargetResponse> {
    let value = event.value;

    let doubled = ctx
        .step(
            Some("double"),
            move |step_ctx| async move {
                step_ctx.info(&format!("Doubling {value}"));
                Ok(value * 2)
            },
            None,
        )
        .await?;

    Ok(InvokeTargetResponse { doubled })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
