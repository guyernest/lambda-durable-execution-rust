# lambda-durable-execution-rust (experimental)

This repository contains an **experimental, community-maintained** Rust SDK for **AWS Lambda Durable Execution** (“durable functions”).

It is **not** an official AWS project. The API and behavior are heavily inspired by (and validated against) the official Durable Execution SDKs for other languages (notably the Node.js/TypeScript implementation), but this crate is developed independently.

## Status / expectations

- **Experimental**: APIs may change and edge cases are still being explored.
- **Compatibility-first**: the goal is to match the Durable Execution service semantics and the official SDK behavior where practical.
- **MSRV**: Rust 1.82 (edition 2021).

## What this SDK provides

The core crate, `lambda-durable-execution-rust`, helps you build replay-safe Lambda workflows by checkpointing durable operations:

- `step(...)`: checkpointed work units (replay returns recorded results)
- `wait(...)`: suspend/resume without paying for idle compute
- `wait_for_callback(...)`: human/external-system approval flows
- `wait_for_condition(...)`: poll until a predicate is true
- `invoke(...)`: durable invocation of another Lambda function
- `parallel(...)` / `map(...)`: fan-out/fan-in patterns with bounded concurrency
- `run_in_child_context(...)`: structured grouping of operations

## Quickstart

Add the dependency:
```toml
[dependencies]
lambda-durable-execution-rust = "0.1.0"
lambda_runtime = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde = { version = "1", features = ["derive"] }
```

Minimal workflow (checkpointed step + durable wait):
```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Event { name: String }

#[derive(Serialize)]
struct Response { message: String }

async fn handler(event: Event, ctx: DurableContextHandle) -> DurableResult<Response> {
    let name = event.name.clone();
    let len = ctx
        .step(Some("name-length"), move |_step_ctx| async move {
            Ok(name.len())
        }, None)
        .await?;
    ctx.wait(Some("wait-10s"), Duration::seconds(10)).await?;
    Ok(Response { message: format!("Hello {}, len={len}", event.name) })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    lambda_runtime::run(with_durable_execution_service(handler, None)).await
}
```

Wait for an external callback (approval-style flow):
```rust,no_run
use lambda_durable_execution_rust::prelude::*;

async fn handler(_event: serde_json::Value, ctx: DurableContextHandle) -> DurableResult<String> {
    let cfg = CallbackConfig::<String>::new().with_timeout(Duration::hours(24));
    let result: String = ctx.wait_for_callback(
        Some("wait-approval"),
        |callback_id, step_ctx| async move {
            step_ctx.info(&format!("Send callback id to external system: {callback_id}"));
            Ok(())
        },
        Some(cfg),
    ).await?;
    Ok(result)
}
```

Durably invoke another Lambda:
```rust,no_run
use lambda_durable_execution_rust::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TargetEvent { value: i32 }
#[derive(Deserialize)]
struct TargetResponse { doubled: i32 }

async fn handler(_event: serde_json::Value, ctx: DurableContextHandle) -> DurableResult<i32> {
    let target_arn = std::env::var("INVOKE_TARGET_FUNCTION")
        .map_err(|_| DurableError::InvalidConfiguration { message: "Missing INVOKE_TARGET_FUNCTION".into() })?;
    let out: TargetResponse = ctx.invoke(Some("invoke-target"), &target_arn, Some(TargetEvent { value: 21 })).await?;
    Ok(out.doubled)
}
```

## Runtime integration

- `with_durable_execution_service(handler, config)` wraps your handler for the Lambda runtime.
- `durable_handler(handler)` exposes a builder API and lets you inject a custom Lambda client or service.
- `DurableExecutionConfig` controls logging, retry policies, and which `LambdaService` implementation is used.

The runtime automatically:
- parses the Durable Execution invocation payload
- initializes the execution context
- checkpoints results and failures
- suspends on waits/callbacks/retries and returns `Pending`

Large handler responses that exceed the Lambda response size limit are checkpointed and returned with an empty payload; the execution result can be reconstructed from the checkpoint state.

## Design notes

- **Replay safety**: step bodies should be deterministic and side‑effect‑free; use durable operations to express side effects.
- **ID hashing**: operation IDs are SHA‑256 hashed and truncated to 128 bits (32 hex chars). This avoids MD5 (often flagged by scanners) while keeping IDs short; the JS SDK uses MD5‑16 and the Python SDK uses BLAKE2b‑64.
- **Serdes**: operations can use custom serializers for individual items and/or full batch results (`map`/`parallel`).

## Project layout

- `Cargo.toml`: SDK crate (`lambda-durable-execution-rust`)
- `src/`: core SDK source
- `examples/`: separate Cargo package with deployable Lambda examples
  - `examples/src/bin/`: example handlers
  - `examples/template.yaml`: SAM template
  - `examples/scripts/`: validation tooling
  - `examples/diagrams/`: generated Mermaid/SVG diagrams

## Module organization

Top-level modules:
- `context`: user-facing durable APIs (`DurableContextHandle`, `StepContext`, `BatchResult`, etc.)
- `checkpoint`: checkpoint queueing and lifecycle management
- `termination`: termination signaling (wait/callback/retry)
- `retry`: retry strategies and presets
- `types`: configuration and SDK wire types
- `runtime`: handler wrappers and execution pipeline
- `error`: SDK error taxonomy

Within `context/durable_context`, each durable operation is split into focused submodules:
- `{step, wait, wait_condition, callback, invoke, child, map, parallel}`
- each operation has `execute` and `replay` logic separated for clarity and testability

Unit tests live alongside their modules in `#[cfg(test)]` blocks and the `context/durable_context/tests/` helpers.

## Build & test

```bash
cargo fmt
cargo test

# Lint
cargo clippy --all-targets --all-features -D warnings

# Run the examples package tests/build checks
cargo test --manifest-path examples/Cargo.toml --all-targets

# Coverage (requires llvm-cov)
cargo llvm-cov --all-features --summary-only
```

## Test utilities (feature-gated)

Mocks for the Lambda Durable Execution API are available behind the `testutils` feature (or automatically in this crate’s own tests):

```toml
[dev-dependencies]
lambda-durable-execution-rust = { version = "0.1", features = ["testutils"] }
```

```rust,no_run
// Requires the `testutils` feature.
#[cfg(feature = "testutils")]
{
    use lambda_durable_execution_rust::mock::{MockCheckpointConfig, MockLambdaService};

    let mock = std::sync::Arc::new(MockLambdaService::new());
    mock.expect_checkpoint(MockCheckpointConfig::default());
}
```

The mock service queues expected responses and records calls, letting you exercise checkpoint and replay flows without AWS.

## Run examples on AWS (SAM)

See `examples/README.md` for:
- deploying `examples/template.yaml` with SAM
- validating all examples with `examples/scripts/validate.py`
- generated Mermaid/SVG diagrams per example

## References

This project is based on the public Durable Execution SDK design and validated against the official SDKs where possible:
- Node.js/TypeScript SDK (local sibling clone): `../aws-durable-execution-sdk-js`
- Python SDK (local sibling clone): `../aws-durable-execution-sdk-python`
