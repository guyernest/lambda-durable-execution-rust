//! Hello World durable workflow.
//!
//! Demonstrates:
//! - `ctx.step()` for deterministic, checkpointed work.
//! - `ctx.wait()` to suspend/resume without paying for idle compute.
//!
//! Event (JSON):
//! - `{ "name": "World" }`
//!
//! Result (JSON):
//! - `{ "message": "...", "name_length": 5 }`
//!
//! This example intentionally includes a 10 second durable wait to make the
//! suspend/resume behavior visible in the execution history.
//! Deployed via `examples/template.yaml`.

use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// Input event for the hello world function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloEvent {
    /// Name to greet.
    pub name: String,
}

/// Response from the hello world function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResponse {
    /// The greeting message.
    pub message: String,
    /// Number of characters in the name.
    pub name_length: usize,
}

/// The durable handler function.
///
/// This function demonstrates:
/// - Receiving a typed event
/// - Using a durable step to perform an operation
/// - Returning a typed response
pub async fn hello_handler(
    event: HelloEvent,
    ctx: DurableContextHandle,
) -> DurableResult<HelloResponse> {
    // Clone the name before the closure to satisfy lifetime requirements
    let name = event.name.clone();

    // Execute a step with automatic checkpointing
    // If the Lambda is interrupted and restarted, this step's result
    // will be replayed from the checkpoint rather than re-executed
    let name_length = ctx
        .step(
            Some("calculate-length"),
            move |step_ctx| {
                // Name is moved into the async block
                async move {
                    step_ctx.info("Calculating name length");
                    Ok(name.len())
                }
            },
            None,
        )
        .await?;

    // Demonstrate a durable wait: this suspends the Lambda and resumes after the duration.
    ctx.wait(Some("wait-10s"), Duration::seconds(10)).await?;

    // Create the response
    let message = format!(
        "Hello, {}! Your name has {} characters.",
        event.name, name_length
    );

    Ok(HelloResponse {
        message,
        name_length,
    })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let handler = with_durable_execution_service(hello_handler, None);
    lambda_runtime::run(handler).await
}
