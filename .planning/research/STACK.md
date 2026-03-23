# Technology Stack

**Project:** Durable Lambda MCP Agent
**Researched:** 2026-03-23
**Overall Confidence:** MEDIUM (web search tools unavailable; versions from Cargo.lock and training data. Crate versions should be verified against crates.io before adding to Cargo.toml.)

## Recommended Stack

### Core Framework (Already Pinned)

These are non-negotiable -- they come from the existing codebase and project constraints.

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `lambda-durable-execution-rust` | 0.1.0 (path dep) | Durable step/map/wait/parallel | This repo -- the entire point |
| `lambda_runtime` | ~1.1.2 | Lambda handler integration | Already in use, `with_durable_execution_service` wraps it |
| `tokio` | 1.x (features: full) | Async runtime | Required by lambda_runtime and durable SDK |
| `serde` / `serde_json` | 1.0 | Serialization | All checkpoint data flows through JsonSerdes |
| `aws-config` | ~1.8.12 | AWS SDK config loading | Already resolved in lockfile |
| `aws-sdk-lambda` | ~1.112.0 | Checkpoint API calls | Already in use by CheckpointManager |

**Confidence:** HIGH -- these are facts from the Cargo.lock, not research.

### Anthropic API Client

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| **Hand-rolled reqwest-based client** | N/A | Anthropic Messages API calls | See rationale below |
| `reqwest` | ~0.12 | HTTP client for Anthropic API | Mature, async, already widely used in Rust Lambda ecosystem |

**Recommendation: Build a thin Anthropic client, do NOT use a third-party `anthropic` crate.**

**Rationale:**

1. **No official Rust SDK from Anthropic.** Anthropic publishes official SDKs for Python, TypeScript, Java, Go, and Kotlin. There is no official Rust SDK as of 2025/2026. Any crate on crates.io is community-maintained.

2. **Community crates are thin wrappers anyway.** The `anthropic` crate on crates.io and alternatives like `misanthropy` are thin reqwest wrappers over the Messages API. They add a dependency without adding meaningful value over hand-rolled code.

3. **The existing LLM caller already has this.** The `call_llm_rust` codebase has an Anthropic transformer with request/response models. Extract and simplify for Claude-only use rather than adding a new dependency.

4. **The Anthropic Messages API is simple.** It is a single POST to `https://api.anthropic.com/v1/messages` with a JSON body. The complexity is in the message types (content blocks, tool_use, tool_result), not the HTTP call. Those types need to be correct for the agent loop regardless of whether a crate provides them.

5. **Checkpoint serialization control.** LLM responses are checkpointed via `ctx.step()`. Having direct control over the serde types (no opaque third-party types) ensures they serialize cleanly within the 750KB checkpoint limit.

**The client needs exactly:**
- `reqwest::Client` with API key header (`x-api-key`, `anthropic-version`)
- Request types: `MessagesRequest` (model, max_tokens, system, messages, tools)
- Response types: `MessagesResponse` with `ContentBlock` enum (Text, ToolUse, ToolResult)
- Stop reason enum: `EndTurn`, `ToolUse`, `MaxTokens`, `StopSequence`

**Confidence:** MEDIUM -- no web search to verify current crate ecosystem. The "no official Rust SDK" claim is based on training data through May 2025. Verify before implementation.

### MCP Client

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `pmcp` | ~2.0.0 | MCP client (list_tools, call_tool) | Already identified in project context as the Rust MCP SDK |

**Rationale:**

1. **Already built and tested.** The project context identifies `pmcp` v2.0.0 at `~/Development/mcp/sdk/rust-mcp-sdk` with Client, HttpTransport, `list_tools()`, `call_tool()`, and middleware (auth, retry, logging).

2. **HTTP/SSE transport is Lambda-compatible.** stdio transport is not viable in Lambda; HTTP/SSE is the correct transport for Lambda-to-MCP-server communication.

3. **Alternative: `rmcp` from the official Rust MCP SDK.** The Model Context Protocol organization published an official Rust SDK (`rmcp` on crates.io). However, since the project already has `pmcp` v2.0.0 battle-tested with the exact features needed (Client, HttpTransport, middleware), switching would add risk for no gain. If `pmcp` proves problematic, `rmcp` is the fallback.

**Key integration pattern:** MCP client connections should be established once during Lambda cold start (outside the handler), then reused across durable execution replays. The `list_tools()` call should be a durable `step()` so tool schemas are cached across replays.

**Confidence:** MEDIUM -- `pmcp` details are from PROJECT.md, not verified from source. `rmcp` existence is from training data.

### AWS SDK (New Dependencies)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `aws-sdk-dynamodb` | ~1.x | Read AgentRegistry configuration | Agent config lives in DynamoDB |
| `aws-sdk-secretsmanager` | ~1.x | Retrieve Anthropic API key | API keys should not be in env vars or config |

**Rationale:** Use matching AWS SDK versions from the same SDK release train as the existing `aws-sdk-lambda ~1.112.0`. All AWS SDK crates share the same `aws-config` and `aws-smithy-*` dependencies, so version alignment avoids duplicate dependency trees.

**Note:** Pin to the same `~1.x` minor version range. Check the latest compatible versions against `aws-sdk-lambda = "~1.112.0"` in the lockfile -- they should be from the same SDK release (January/February 2026 timeframe based on the 1.112 version).

**Confidence:** HIGH -- AWS SDK for Rust dependency patterns are well-established.

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `reqwest` | ~0.12 | HTTP client for Anthropic API | All LLM API calls |
| `tracing` | 0.1 | Structured logging | Already in use |
| `tracing-subscriber` | 0.3 | Log formatting with env-filter | Already in use |
| `thiserror` | 2.0 | Error type definitions | Already in use by SDK |
| `chrono` | 0.4 | Timestamps in agent messages | Already in use |
| `uuid` | 1.0 (features: v4) | Request IDs, correlation | Already in use |

**Confidence:** HIGH -- all either already in Cargo.toml or are standard Rust ecosystem choices.

### Build and Deploy

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `cargo-lambda` | latest | Cross-compile for Lambda ARM64 | Used by SAM via `BuildMethod: rust-cargolambda` |
| AWS SAM CLI | latest | Build and deploy | Existing pattern in `examples/template.yaml` |
| `just` | latest | Task runner for build/test/deploy | Per user preference (justfile over Makefile) |

**Confidence:** HIGH -- existing deployment pattern in the repo.

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

```
lambda-durable-execution-rust (path = "..")
  +-- lambda_runtime
  +-- tokio
  +-- serde / serde_json
  +-- aws-sdk-lambda (checkpoint)
  +-- aws-config

pmcp (MCP client)
  +-- reqwest (HTTP/SSE transport)
  +-- serde / serde_json

reqwest (Anthropic API client)

aws-sdk-dynamodb (AgentRegistry)
aws-sdk-secretsmanager (API keys)

tracing / tracing-subscriber (logging)
thiserror (error types)
```

Note: `reqwest` is likely already a transitive dependency of `pmcp`. Adding it as a direct dependency for the Anthropic client should not increase binary size.

## Agent-Specific Types to Build (Not External Dependencies)

These are not crate dependencies but in-repo modules that form the agent's core:

### Anthropic Message Types
```rust
// Minimal set needed for the agent loop:
struct MessagesRequest { model, max_tokens, system, messages, tools, ... }
struct MessagesResponse { id, content, stop_reason, usage, ... }
enum ContentBlock { Text { text }, ToolUse { id, name, input }, ToolResult { tool_use_id, content } }
enum StopReason { EndTurn, ToolUse, MaxTokens, StopSequence }
struct ToolDefinition { name, description, input_schema }
```

### MCP-to-Claude Tool Translation
```rust
// Convert pmcp::Tool to Anthropic ToolDefinition
fn mcp_tool_to_claude_tool(mcp_tool: &McpTool) -> ToolDefinition { ... }

// Convert Claude ToolUse to pmcp::call_tool arguments
fn claude_tool_use_to_mcp_call(tool_use: &ToolUse) -> McpCallToolRequest { ... }
```

### Agent Configuration
```rust
// Read from DynamoDB AgentRegistry
struct AgentConfig {
    system_prompt: String,
    model: String,           // e.g., "claude-sonnet-4-20250514"
    mcp_servers: Vec<McpServerConfig>,
    max_iterations: u32,
    temperature: f64,
    max_tokens: u32,
}
```

## Installation

```toml
# Cargo.toml for the agent binary (in examples/ or a new crate)
[dependencies]
# Core (already available)
lambda-durable-execution-rust = { path = ".." }
lambda_runtime = "~1.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# AWS SDK
aws-config = "~1.8"
aws-sdk-dynamodb = "~1.112"
aws-sdk-secretsmanager = "~1.112"

# MCP Client
pmcp = "2.0"

# HTTP Client (for Anthropic API)
reqwest = { version = "~0.12", features = ["json", "rustls-tls"], default-features = false }

# Error handling
thiserror = "2.0"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

**Important:** The `aws-sdk-dynamodb` and `aws-sdk-secretsmanager` version numbers above are estimates. They should match the same SDK release train as `aws-sdk-lambda = "~1.112.0"`. Verify actual latest versions on crates.io.

**reqwest feature flags:**
- `json` -- enables `.json()` body serialization
- `rustls-tls` -- uses rustls instead of OpenSSL (avoids OpenSSL linking issues on Lambda AL2023)
- `default-features = false` -- excludes the default `default-tls` (OpenSSL) feature

## Version Verification Checklist

These versions need verification against crates.io before implementation (web search was unavailable during research):

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
