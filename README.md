# lambda-durable-execution-rust (experimental)

`lambda-durable-execution-rust` is a community-driven SDK that brings the power of **AWS Lambda Durable Execution** to the Rust ecosystem. It enables developers to build complex, stateful serverless workflows using familiar async Rust patterns, with built-in support for automatic checkpointing, durable timers, parallel execution, and reliable retries—eliminating the need for external orchestration infrastructure.

> [!NOTE]
> This repository contains an **experimental, community-maintained** Rust SDK for **AWS Lambda Durable Execution** (“durable functions”).
> It is **not** an official AWS project. The API and behavior are heavily inspired by (and validated against) the official and publicly available Durable Execution SDKs for TypeScript and Python, but this crate is developed independently, and most of the implementation was drafted with the help of AI assistants (see [AGENTS.md](AGENTS.md) and [CLAUDE.md](CLAUDE.md)) and has only been exercised in my own workloads. The official AWS SDK will likely be released in the near future; please consider this crate as a stopgap solution for Rust users who want to experiment with Durable Execution today.



## Status / expectations

- **Experimental**: APIs may change and edge cases are still being explored.
- **Compatibility-first**: the goal is to match the Durable Execution service semantics and the official SDK behavior where practical.
- **MSRV**: Rust 1.88 (edition 2021).

## Documentation

- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Internal architecture, operation diagrams, and code examples
- **[examples/](examples/)** - Deployable Lambda examples with SAM template

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

## Durable Operations

The SDK provides the following durable operations, each with automatic checkpointing and replay support. See [ARCHITECTURE.md](ARCHITECTURE.md#operation-types) for detailed diagrams and code examples.

| Operation | Description | Examples |
|-----------|-------------|----------|
| `step` | Checkpointed work units with optional retry | [`hello_world`](examples/src/bin/hello_world/main.rs), [`step_retry`](examples/src/bin/step_retry/main.rs) |
| `wait` | Suspend without compute cost | [`hello_world`](examples/src/bin/hello_world/main.rs) |
| `wait_for_callback` | External system integration (approvals, webhooks) | [`callback_example`](examples/src/bin/callback_example/main.rs), [`wait_for_callback_heartbeat`](examples/src/bin/wait_for_callback_heartbeat/main.rs) |
| `wait_for_condition` | Poll until condition is satisfied | [`wait_for_condition`](examples/src/bin/wait_for_condition/main.rs) |
| `invoke` | Durable Lambda invocation | [`invoke_caller`](examples/src/bin/invoke_caller/main.rs), [`invoke_target`](examples/src/bin/invoke_target/main.rs) |
| `parallel` | Concurrent branch execution | [`parallel`](examples/src/bin/parallel/main.rs), [`parallel_first_successful`](examples/src/bin/parallel_first_successful/main.rs) |
| `parallel_named` | Named concurrent branches | [`parallel_named`](examples/src/bin/parallel_named/main.rs) |
| `map` | Process items concurrently | [`map_operations`](examples/src/bin/map_operations/main.rs), [`map_with_failure_tolerance`](examples/src/bin/map_with_failure_tolerance/main.rs) |
| `run_in_child_context` | Grouped operations | [`child_context`](examples/src/bin/child_context/main.rs), [`block_example`](examples/src/bin/block_example/main.rs) |

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

### AWS Documentation

For the official AWS Lambda Durable Execution service documentation, see:
- [Lambda durable functions](https://docs.aws.amazon.com/lambda/latest/dg/durable-functions.html) - Overview and concepts
- [Durable execution SDK](https://docs.aws.amazon.com/lambda/latest/dg/durable-execution-sdk.html) - SDK usage guide
- [Configuration](https://docs.aws.amazon.com/lambda/latest/dg/durable-configuration.html) - Timeouts and retention
- [Best practices](https://docs.aws.amazon.com/lambda/latest/dg/durable-best-practices.html) - Determinism and idempotency

### Official SDKs

This project is based on the public Durable Execution SDK design and validated against the official SDKs where possible:
- [aws/aws-durable-execution-sdk-js](https://github.com/aws/aws-durable-execution-sdk-js) - Node.js/TypeScript SDK
- [aws/aws-durable-execution-sdk-python](https://github.com/aws/aws-durable-execution-sdk-python) - Python SDK
