// SPDX-License-Identifier: Apache-2.0
// Ported from aws-durable-execution-sdk-python-testing examples.
// Source: examples/src/block_example/block_example.py
// See NOTICE for attribution.

//! Nested child contexts (“blocks”) example.
//!
//! Demonstrates:
//! - `ctx.run_in_child_context()` nesting and parent/child operation hierarchy.
//! - Mixing steps and waits inside nested contexts.
//!
//! Event (JSON): `{}` (ignored)
//! Result (JSON): `{ "nestedStep": "nested step result", "nestedBlock": "nested block result" }`
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
    ctx.run_in_child_context(
        Some("parent_block"),
        |parent_ctx| async move {
            let nested_result: String = parent_ctx
                .step(
                    Some("nested_step"),
                    |_step_ctx| async move { Ok("nested step result".to_string()) },
                    None,
                )
                .await?;

            let nested_block_result: String = parent_ctx
                .run_in_child_context(
                    Some("nested_block"),
                    |nested_ctx| async move {
                        nested_ctx.wait(None, Duration::seconds(1)).await?;
                        Ok("nested block result".to_string())
                    },
                    None,
                )
                .await?;

            Ok(json!({
                "nestedStep": nested_result,
                "nestedBlock": nested_block_result,
            }))
        },
        None,
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
