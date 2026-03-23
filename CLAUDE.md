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

<!-- GSD:project-start source:PROJECT.md -->
## Project

**Durable Lambda MCP Agent**

A Rust-based Durable Lambda agent that replaces the existing Step Functions agent pattern. The agent acts as an MCP client, connecting to MCP servers for tool discovery and execution, while using the existing Rust LLM caller for multi-provider LLM support. Managed via the existing AgentRegistry and admin UI alongside Step Functions agents.

**Core Value:** A single Durable Lambda replaces the entire Step Functions orchestration — the agent loop is plain Rust code with checkpointed LLM calls and MCP tool executions, no state machine definition required.

### Constraints

- **Tech stack**: Rust (edition 2021, MSRV 1.88), AWS Lambda with Durable Execution, SAM for deployment
- **Dependencies**: Must use `lambda-durable-execution-rust` (this crate), `pmcp` (MCP SDK), and reuse Anthropic-specific code from the existing Rust LLM caller
- **MCP transport**: HTTP/SSE for Lambda-to-MCP-server communication (stdio not viable in Lambda)
- **AgentRegistry compatibility**: Must read from the existing DynamoDB table schema; new fields (MCP server endpoints) should be additive, not breaking
- **Checkpoint limits**: 750KB per checkpoint batch — message histories for long conversations must stay within bounds or be managed
<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->
## Technology Stack

## Recommended Stack
### Core Framework (Already Pinned)
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `lambda-durable-execution-rust` | 0.1.0 (path dep) | Durable step/map/wait/parallel | This repo -- the entire point |
| `lambda_runtime` | ~1.1.2 | Lambda handler integration | Already in use, `with_durable_execution_service` wraps it |
| `tokio` | 1.x (features: full) | Async runtime | Required by lambda_runtime and durable SDK |
| `serde` / `serde_json` | 1.0 | Serialization | All checkpoint data flows through JsonSerdes |
| `aws-config` | ~1.8.12 | AWS SDK config loading | Already resolved in lockfile |
| `aws-sdk-lambda` | ~1.112.0 | Checkpoint API calls | Already in use by CheckpointManager |
### Anthropic API Client
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| **Hand-rolled reqwest-based client** | N/A | Anthropic Messages API calls | See rationale below |
| `reqwest` | ~0.12 | HTTP client for Anthropic API | Mature, async, already widely used in Rust Lambda ecosystem |
- `reqwest::Client` with API key header (`x-api-key`, `anthropic-version`)
- Request types: `MessagesRequest` (model, max_tokens, system, messages, tools)
- Response types: `MessagesResponse` with `ContentBlock` enum (Text, ToolUse, ToolResult)
- Stop reason enum: `EndTurn`, `ToolUse`, `MaxTokens`, `StopSequence`
### MCP Client
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `pmcp` | ~2.0.0 | MCP client (list_tools, call_tool) | Already identified in project context as the Rust MCP SDK |
### AWS SDK (New Dependencies)
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `aws-sdk-dynamodb` | ~1.x | Read AgentRegistry configuration | Agent config lives in DynamoDB |
| `aws-sdk-secretsmanager` | ~1.x | Retrieve Anthropic API key | API keys should not be in env vars or config |
### Supporting Libraries
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `reqwest` | ~0.12 | HTTP client for Anthropic API | All LLM API calls |
| `tracing` | 0.1 | Structured logging | Already in use |
| `tracing-subscriber` | 0.3 | Log formatting with env-filter | Already in use |
| `thiserror` | 2.0 | Error type definitions | Already in use by SDK |
| `chrono` | 0.4 | Timestamps in agent messages | Already in use |
| `uuid` | 1.0 (features: v4) | Request IDs, correlation | Already in use |
### Build and Deploy
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `cargo-lambda` | latest | Cross-compile for Lambda ARM64 | Used by SAM via `BuildMethod: rust-cargolambda` |
| AWS SAM CLI | latest | Build and deploy | Existing pattern in `examples/template.yaml` |
| `just` | latest | Task runner for build/test/deploy | Per user preference (justfile over Makefile) |
## Alternatives Considered
| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Anthropic client | Hand-rolled (reqwest) | `anthropic` crate | No official SDK; community crates are thin wrappers with version lag. Direct control over serde types needed for checkpoint compatibility |
| Anthropic client | Hand-rolled (reqwest) | `aws-sdk-bedrockruntime` (Bedrock) | Adds Bedrock dependency; PoC targets direct Anthropic API per PROJECT.md. Bedrock support can be added later via provider abstraction |
| MCP client | `pmcp` v2.0.0 | `rmcp` (official MCP Rust SDK) | `pmcp` already built and integrated in the project ecosystem. Switch only if `pmcp` proves inadequate |
| MCP client | `pmcp` v2.0.0 | Raw HTTP/SSE | MCP protocol has non-trivial session management; SDK handles this correctly |
| HTTP client | `reqwest` | `hyper` directly | reqwest provides higher-level API (JSON body, headers, timeouts) with less boilerplate. hyper is already a transitive dep |
| HTTP client | `reqwest` | `aws-smithy-http` | Smithy HTTP is internal to AWS SDK; not designed for general HTTP client use |
| Error handling | `thiserror` | `anyhow` | `thiserror` is already in use; agent errors should be typed (LlmError, McpError, ConfigError) not opaque |
| Serialization | `serde_json` | `simd-json` | No perf need; checkpoint payloads are <750KB; standard serde_json matches SDK |
| Config | DynamoDB direct | SSM Parameter Store | AgentRegistry is already in DynamoDB; adding SSM would split config across two services |
## Dependency Graph (New Agent Binary)
## Agent-Specific Types to Build (Not External Dependencies)
### Anthropic Message Types
### MCP-to-Claude Tool Translation
### Agent Configuration
## Installation
# Cargo.toml for the agent binary (in examples/ or a new crate)
# Core (already available)
# AWS SDK
# MCP Client
# HTTP Client (for Anthropic API)
# Error handling
# Logging
- `json` -- enables `.json()` body serialization
- `rustls-tls` -- uses rustls instead of OpenSSL (avoids OpenSSL linking issues on Lambda AL2023)
- `default-features = false` -- excludes the default `default-tls` (OpenSSL) feature
## Version Verification Checklist
| Crate | Claimed Version | Verify | Risk if Wrong |
|-------|----------------|--------|---------------|
| `pmcp` | 2.0.0 | crates.io | May not exist on crates.io -- could be a local/git dependency |
| `reqwest` | ~0.12 | crates.io | Low risk -- 0.12 was stable by mid-2025 |
| `aws-sdk-dynamodb` | ~1.112 | crates.io | Version number is estimated from aws-sdk-lambda; may differ |
| `aws-sdk-secretsmanager` | ~1.112 | crates.io | Same as above |
| `lambda_runtime` | ~1.1 | Cargo.lock shows 1.1.2 | LOW risk -- confirmed in lockfile |
## Sources
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/Cargo.toml` -- current SDK dependencies
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/Cargo.lock` -- resolved versions
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/.planning/PROJECT.md` -- project context, existing assets
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/template.yaml` -- SAM deployment pattern
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/types/serdes.rs` -- checkpoint serialization interface
- Training data (May 2025 cutoff) -- Anthropic API docs, MCP protocol, Rust crate ecosystem. Marked as MEDIUM confidence where applicable.
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

Conventions not yet established. Will populate as patterns emerge during development.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd:quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd:debug` for investigation and bug fixing
- `/gsd:execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd:profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
