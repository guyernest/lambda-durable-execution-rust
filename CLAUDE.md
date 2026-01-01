# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is an **experimental, community-maintained** Rust SDK for AWS Lambda Durable Execution. It provides replay-safe Lambda workflows by checkpointing durable operations like `step`, `wait`, `wait_for_callback`, `invoke`, `parallel`, `map`, and `run_in_child_context`.

## Build, Test, and Lint Commands

```bash
# SDK crate
cargo build                  # Build the SDK
cargo check                  # Fast typecheck
cargo test                   # Run SDK unit tests + doc-tests
cargo test --doc             # Doc-tests only
cargo fmt                    # Format code
cargo fmt --check            # CI-friendly format check
cargo clippy --all-targets --all-features -D warnings  # Lint (keep warnings at zero)

# Examples package (separate Cargo.toml in examples/)
cargo test --manifest-path examples/Cargo.toml --all-targets
cargo build --manifest-path examples/Cargo.toml --all-targets

# Deploy examples with SAM
sam build -t examples/template.yaml --beta-features
sam deploy -t examples/template.yaml --guided --region us-east-1 --stack durable-rust

# Validate deployed examples and regenerate diagrams
uv run examples/scripts/validate.py \
  --region us-east-1 --stack durable-rust \
  --out examples/.durable-validation \
  --diagrams-out examples/diagrams \
  --mermaid --timeout-seconds 240
```

## Architecture

For detailed architecture documentation with diagrams, see [ARCHITECTURE.md](ARCHITECTURE.md).

### Module Structure (`src/`)

- **`context/`**: Core `DurableContextHandle` and `ExecutionContext` types. The handle is the main interface for durable operations (`step`, `wait`, `wait_for_callback`, `invoke`, `parallel`, `map`, `run_in_child_context`).
- **`checkpoint/`**: `CheckpointManager` persists operation state to AWS Lambda's checkpoint service. Uses async batching with a 750KB per-batch safeguard.
- **`termination/`**: `TerminationManager` coordinates Lambda suspension via tokio watch channels. Handles wait, callback, invoke, and retry termination reasons.
- **`runtime/`**: Handler wrapper (`with_durable_execution_service`, `durable_handler`) that integrates with `lambda_runtime`. Parses durable input, sets up context, manages lifecycle via `tokio::select!` racing.
- **`retry/`**: Retry strategies (`ExponentialBackoff`, `ConstantDelay`, `FixedRetry`, `NoRetry`) with jitter support. Presets available in `retry::presets`.
- **`types/`**: Configuration types (`StepConfig`, `CallbackConfig`, `ParallelConfig`, `MapConfig`, `Duration`), invocation types, serialization helpers (`Serdes`), and logging.
- **`error/`**: `DurableError` enum and `DurableResult<T>` alias. Errors are categorized by recoverability.

### Key Patterns

- **Handler signature**: `async fn(Event, DurableContextHandle) -> DurableResult<Response>` where `Event: DeserializeOwned` and `Response: Serialize`.
- **Prelude import**: `use lambda_durable_execution_rust::prelude::*;` provides `DurableContextHandle`, `DurableResult`, `DurableError`, `Duration`, config types, retry strategies, and `Arc`.
- **Determinism requirement**: Handlers must be deterministic — side effects belong inside `ctx.step()` to preserve replay correctness.

### Examples (`examples/`)

A separate Cargo package with deployable Lambda binaries under `examples/src/bin/`. Each example demonstrates a durable execution pattern (steps, waits, callbacks, parallel/map, child contexts, retry strategies). SAM template at `examples/template.yaml`. Diagrams generated from real execution history live in `examples/diagrams/`.

## Coding Conventions

- Rust edition 2021, MSRV 1.88
- Use rustfmt defaults; run `cargo fmt` before committing
- Public APIs require rustdoc (`missing_docs` is warned)
- Tests live in `#[cfg(test)] mod tests { ... }` blocks; name tests `test_*`; use `#[tokio::test]` for async
- Keep rustdoc examples compiling: prefer ` ```rust,no_run` over `ignore`

## Commit Guidelines

Follow Conventional Commits when possible (e.g., `feat(retry): add jitter`, `fix(runtime): handle empty input`). PRs should pass `cargo fmt --check`, `cargo clippy ... -D warnings`, `cargo test`, and `cargo test --manifest-path examples/Cargo.toml --all-targets`.
