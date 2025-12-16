# lambda-durable-execution-rust (experimental)

This repository contains an **experimental, community-maintained** Rust SDK for **AWS Lambda Durable Execution** (“durable functions”).

It is **not** an official AWS project. The API and behavior are heavily inspired by (and validated against) the official Durable Execution SDKs for other languages (notably the Node.js/TypeScript implementation), but this crate is developed independently.

## What this SDK provides

The core crate, `lambda-durable-execution-rust`, helps you build replay-safe Lambda workflows by checkpointing durable operations:

- `step(...)`: checkpointed work units (replay returns recorded results)
- `wait(...)`: suspend/resume without paying for idle compute
- `wait_for_callback(...)`: human/external-system approval flows
- `invoke(...)`: durable invocation of another Lambda function
- `parallel(...)` / `map(...)`: fan-out/fan-in patterns with bounded concurrency
- `run_in_child_context(...)`: structured grouping of operations

## Status / expectations

- **Experimental**: APIs may change and edge cases are still being explored.
- **Compatibility-first**: the goal is to match the Durable Execution service semantics and the official SDK behavior where practical.

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
```rust,ignore
use lambda_durable_execution_rust::prelude::*;
use lambda_durable_execution_rust::runtime::with_durable_execution_service;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Event { name: String }

#[derive(Serialize)]
struct Response { message: String }

async fn handler(event: Event, ctx: DurableContextHandle) -> DurableResult<Response> {
    let len = ctx.step(Some("name-length"), move |_step_ctx| async move { Ok(event.name.len()) }, None).await?;
    ctx.wait(Some("wait-10s"), Duration::seconds(10)).await?;
    Ok(Response { message: format!("Hello {}, len={len}", event.name) })
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    lambda_runtime::run(with_durable_execution_service(handler, None)).await
}
```

Wait for an external callback (approval-style flow):
```rust,ignore
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
```rust,ignore
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

## Layout

- `src/`: the SDK crate source
- `examples/`: deployable Lambda examples + SAM template + validator script
- `examples/scripts/validate.py`: invokes each deployed example and generates diagrams from execution history

## Build & test

```bash
cargo fmt
cargo test

# Run the examples package tests/build checks
cargo test --manifest-path examples/Cargo.toml --all-targets
```

## Run examples on AWS (SAM)

See `examples/README.md` for:
- deploying `examples/template.yaml` with SAM
- validating all examples with `examples/scripts/validate.py`
- generated Mermaid/SVG diagrams per example

## References

This project is based on the public Durable Execution SDK design and validated against the official SDKs where possible:
- Node.js/TypeScript SDK (local sibling clone): `../aws-durable-execution-sdk-js`
- Python SDK (local sibling clone): `../aws-durable-execution-sdk-python`
