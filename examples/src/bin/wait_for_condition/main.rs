// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/wait_for_condition/wait_for_condition.py
// See NOTICE for attribution.

//! Wait-for-condition (polling) example.
//!
//! Demonstrates:
//! - `ctx.wait_for_condition()` to repeatedly run a step until a stop condition is reached.
//! - Returning `WaitConditionDecision::Continue { delay }` to suspend between polls.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON number): `3`
//!
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use tracing_subscriber::EnvFilter;

pub async fn handler(_event: serde_json::Value, ctx: DurableContextHandle) -> DurableResult<i32> {
    let wait_strategy = Arc::new(|state: &i32, _attempt: u32| {
        if *state >= 3 {
            WaitConditionDecision::Stop
        } else {
            WaitConditionDecision::Continue {
                delay: Duration::seconds(1),
            }
        }
    });

    let config = WaitConditionConfig::new(0, wait_strategy);

    ctx.wait_for_condition(
        Some("wait_for_condition"),
        |state: i32, _step_ctx: StepContext| async move { Ok(state + 1) },
        config,
    )
    .await
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let svc = with_durable_execution_service(handler, None);
    lambda_runtime::run(svc).await
}
