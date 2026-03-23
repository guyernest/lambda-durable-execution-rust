# Project Research Summary

**Project:** Durable Lambda MCP Agent
**Domain:** AI Agent Orchestration (Durable Lambda with MCP Client + LLM Caller)
**Researched:** 2026-03-23
**Confidence:** MEDIUM (HIGH for SDK internals from source code; MEDIUM for external crate versions and Anthropic API details from training data without web verification)

## Executive Summary

The Durable Lambda MCP Agent is a single Rust Lambda binary that implements an LLM agent loop -- call Claude, execute MCP tools, repeat -- using the durable execution SDK for replay-safe checkpointing. It replaces the existing Step Functions agent architecture with dramatically simpler deployment (one Lambda, one SAM resource) while gaining automatic suspension/resumption, per-step retry, and checkpoint-based conversation durability for free. The stack is largely predetermined: the durable execution SDK provides the execution framework, a hand-rolled reqwest client calls the Anthropic Messages API (no third-party Anthropic crate -- the API is a single POST endpoint), and pmcp v2.0 provides MCP client connectivity.

The recommended architecture is a two-phase handler: a setup phase (load config from DynamoDB, connect to MCP servers, discover tools) followed by an agent loop (LLM call as `ctx.step()`, tool execution as `ctx.map()`, repeat until `end_turn`). The critical architectural insight is that MCP connections are ephemeral (TCP sockets cannot survive Lambda suspension) while MCP operation *results* are durable (checkpointed via steps). Tool discovery results are cached on first invocation; tool calls create fresh connections but only execute on the live path (cached results are returned instantly during replay).

The most dangerous pitfalls are all determinism-related. The durable SDK replays the handler from scratch on every invocation, using a global operation counter to match steps to cached results. Non-deterministic code between steps (timestamps, random values, external reads) causes the counter to diverge, silently re-executing LLM calls (wasting money) or failing entirely. The mitigation is strict discipline: every piece of state the loop branches on must come from a `ctx.step()` result, tool calls always use `ctx.map()` for counter isolation, and message history is reconstructed from individual step results rather than stored monolithically. The 750KB checkpoint limit is comfortable for 10-iteration runs (~225KB typical) but needs monitoring for longer conversations.

## Key Findings

### Recommended Stack

The stack combines three existing codebases with minimal new dependencies. The durable SDK, Lambda runtime, tokio, serde, and AWS SDK are already in use. New additions are limited to reqwest (Anthropic HTTP client), pmcp (MCP client), aws-sdk-dynamodb (config), and aws-sdk-secretsmanager (API keys).

**Core technologies:**
- **lambda-durable-execution-rust** (this crate): Provides `step()`, `map()`, `wait()`, checkpoint management, and replay infrastructure
- **Hand-rolled Anthropic client** (reqwest ~0.12): Single POST to Messages API; direct control over serde types for checkpoint compatibility; no official Rust SDK exists
- **pmcp v2.0**: MCP client with HttpTransport for tool discovery and execution; already built and tested in project ecosystem
- **aws-sdk-dynamodb / aws-sdk-secretsmanager**: Config from AgentRegistry, API keys from Secrets Manager; version-aligned with existing aws-sdk-lambda ~1.112.0
- **reqwest with rustls-tls**: Avoids OpenSSL linking issues on Lambda AL2023; json feature for body serialization

**Critical version note:** pmcp may not be on crates.io -- could require a git or path dependency. AWS SDK addon versions are estimated and need verification.

### Expected Features

**Must have (table stakes):**
- Agentic loop (LLM call -> tool calls -> repeat) with each LLM call as a durable `ctx.step()`
- MCP server connection, tool discovery via `list_tools()`, tool execution via `call_tool()`
- Tool schema translation from MCP format (`inputSchema`) to Claude API format (`input_schema`)
- AgentRegistry config loading (system prompt, model, MCP endpoints, parameters)
- LLM API call with retry (429/529 retryable, 400/401 not)
- Message history assembly from checkpointed step results
- Max iterations guard to prevent runaway costs
- MCP tool call error handling (pass errors to LLM as tool_result with is_error, not as step failures)
- Deterministic handler design (all side effects inside `step()` closures)

**Should have (differentiators):**
- Parallel tool execution via `ctx.map()` -- when LLM returns multiple tool_use blocks, execute concurrently
- Token counting from Anthropic usage field -- cost visibility and context window awareness
- Structured iteration logging using SDK's checkpoint trace as observability
- Iteration metadata in response (iteration count, tokens used, tools called)

**Defer (v2+):**
- Context window overflow detection and conversation summarization (high complexity)
- Streaming LLM responses (breaks agent loop's need for complete responses)
- Human-in-the-loop approval flows (SDK supports it via `wait_for_callback()` but UI is separate project)
- Multi-provider LLM support, agent-to-agent delegation, dynamic tool loading
- Image/file content in tool results (text-only for PoC)

### Architecture Approach

The agent is a single durable handler binary at `examples/src/bin/mcp_agent/` with six modules: main.rs (Lambda entry point), handler.rs (durable loop), config.rs (DynamoDB loader), llm.rs (Anthropic client), mcp.rs (tool discovery/execution), and messages.rs (conversation state). Message history is NOT stored as a monolithic checkpoint -- it is reconstructed from individual step results during replay, distributing storage across many small operations.

**Major components:**
1. **Agent Handler** -- Orchestrates the durable loop: config -> MCP connect -> [LLM call -> tool execute -> repeat]
2. **LLM Caller** -- Anthropic Messages API client with typed request/response, error classification (retryable vs not)
3. **MCP Client Manager** -- Connects to servers, discovers tools, executes tool calls with fresh connections per use
4. **Config Loader** -- Reads AgentConfig from DynamoDB AgentRegistry, checkpointed for replay consistency
5. **Message History** -- Typed message builder enforcing Anthropic's strict role alternation and tool_use/tool_result pairing

### Critical Pitfalls

1. **Non-deterministic branching outside steps** -- Any code path divergence between invocations (timestamps, randomness, external reads outside `step()`) causes the global operation counter to diverge, silently re-executing completed work or failing entirely. Prevention: every branching value must come from a `ctx.step()` result.

2. **Checkpoint payload size exceeded** -- Per-operation limit is 256KB, per-batch is 750KB. Naive approach of checkpointing full message history as one blob fails by iteration 5-10. Prevention: checkpoint only each individual LLM response and tool result, reconstruct full history from cached pieces.

3. **MCP connection lifecycle mismatch** -- TCP connections cannot survive Lambda suspension. Attempting to reuse or checkpoint connections fails. Prevention: connect-per-use inside step closures; only serializable results are checkpointed.

4. **Unbounded operation growth** -- A 20-iteration agent with 3 tools per iteration creates 80+ operations. Replay overhead grows linearly. Prevention: use `ctx.map()` for tool fan-out (child context isolation), consider `run_in_child_context` per iteration, set reasonable max_iterations (25-50).

5. **Anthropic message format ordering violations** -- Claude API requires strict user/assistant alternation and tool_use/tool_result ID pairing. Breaking this during message reconstruction causes 400 errors. Prevention: typed message builder that enforces invariants; validate before sending.

## Implications for Roadmap

Based on combined research, the project breaks into 5 phases with clear dependency ordering.

### Phase 1: Foundation Types and Anthropic Client
**Rationale:** Everything depends on correct message types. The LLM response types define what gets checkpointed, what the loop branches on, and what tool results look like. Getting these wrong cascades into every other phase.
**Delivers:** Anthropic Messages API client (reqwest-based), all request/response types (MessagesRequest, MessagesResponse, ContentBlock enum, Tool definition, StopReason enum), error classification (retryable vs non-retryable HTTP status codes).
**Addresses features:** LLM API call with retry, LLM error classification.
**Avoids pitfalls:** Type mismatches during checkpoint serialization (Pitfall 2), message ordering errors (Pitfall 6).

### Phase 2: Configuration and MCP Integration
**Rationale:** The agent loop needs config (system prompt, model, MCP endpoints) and tools before it can call the LLM. Config loading and MCP integration can be built in parallel since they have no mutual dependencies, but both must be complete before Phase 3.
**Delivers:** AgentConfig type and DynamoDB loader, MCP client connection via pmcp HttpTransport, tool discovery (`list_tools()`), tool execution (`call_tool()`), MCP-to-Claude tool schema translation, tool routing map (tool name -> server).
**Addresses features:** AgentRegistry config loading, MCP server connection, tool schema translation, system prompt/model/temperature from config, MCP connection failure handling.
**Avoids pitfalls:** Config changes mid-execution (Pitfall 12 -- wrap in durable step), MCP connection lifecycle mismatch (Pitfall 3 -- connect-per-use pattern), cold start amplification (Pitfall 8 -- lazy/parallel connections), schema translation fidelity (Pitfall 9 -- test with real schemas).

### Phase 3: Core Agent Loop
**Rationale:** This is the core value proposition but depends on Phases 1 and 2 being solid. The loop wires together LLM calling, tool execution, and message history management inside the durable execution framework.
**Delivers:** Complete durable agent handler with `step()` for LLM calls and `map()` for tool execution, deterministic message history reconstruction from step results, max iterations guard, final response extraction, Lambda entry point with `with_durable_execution_service()`.
**Addresses features:** Agentic loop core, message history assembly, tool result formatting, max iterations guard, deterministic handler design.
**Avoids pitfalls:** Non-deterministic branching (Pitfall 1 -- all branching on step results), operation ID divergence (Pitfall 5 -- always use `ctx.map()` for tools), checkpoint size (Pitfall 2 -- incremental message storage).

### Phase 4: Production Hardening
**Rationale:** A working sequential agent loop must exist before adding parallel execution, structured error handling, and observability. These features improve quality and performance but are not required for a functional PoC.
**Delivers:** Parallel tool execution via `ctx.map()` with bounded concurrency, MCP tool error vs transport error distinction, retry strategies on tool call steps, token counting from Anthropic usage field, structured logging with tracing spans.
**Addresses features:** Parallel tool execution, MCP tool call error handling, graceful agent failure, token counting, per-step structured logging.
**Avoids pitfalls:** Server unavailability (Pitfall 7 -- retry strategies + error classification), error propagation asymmetry (Pitfall 13 -- ToolError vs TransportError enum), replay logging noise (Pitfall 11 -- mode-aware logging + replay flag).

### Phase 5: Deployment and Validation
**Rationale:** Deployment is the last phase because it requires a working binary, but SAM template setup and permissions configuration are straightforward following existing example patterns.
**Delivers:** SAM template with DurableConfig, IAM roles (DynamoDB read, Secrets Manager read, VPC for MCP servers), justfile tasks for build/test/deploy, end-to-end validation with real MCP servers, execution trace diagrams.
**Addresses features:** Deployable via SAM template matching existing patterns.
**Avoids pitfalls:** Secrets management (Pitfall 10 -- fetch API key at init, never checkpoint), timeout configuration (DurableExecutionConfig timeout appropriate for agent workloads).

### Phase Ordering Rationale

- **Phase 1 before Phase 2:** Message types are used in tool translation (MCP schemas map to Claude ToolDefinition) and config (model names map to API parameters). Getting types right first prevents cascading rework.
- **Phase 2 before Phase 3:** The agent loop calls `list_tools()` and reads config for system prompt and model. These must exist before the loop can run.
- **Phase 3 before Phase 4:** Parallel tool execution and error refinement layer on top of a working sequential loop. Do not optimize what does not yet work.
- **Phase 4 before Phase 5:** Hardening fixes issues that would be painful to debug in a deployed environment.
- **Phase 5 last:** SAM deployment is mechanical once the binary works locally.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1:** Anthropic API message types need verification against current docs. Training data (May 2025) may have stale field names or missing content block types. Test against the real API early.
- **Phase 2:** pmcp v2.0 API needs verification. The crate may not be on crates.io; may need a git or path dependency. Also: the `rmcp` crate (official MCP Rust SDK) exists as a fallback if pmcp proves problematic. Inspect pmcp source before building the integration layer.

Phases with standard patterns (skip research-phase):
- **Phase 3:** The durable SDK's `step()` and `map()` APIs are thoroughly documented in source code and examples. The agent loop pattern is well-established.
- **Phase 4:** Retry configuration, error handling patterns, and structured logging are all well-documented in the SDK.
- **Phase 5:** SAM deployment follows the existing pattern in `examples/template.yaml`. No research needed.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack - Core SDK | HIGH | Directly verified from Cargo.toml, Cargo.lock, and source code |
| Stack - Anthropic Client | MEDIUM | No official Rust SDK claim from training data (May 2025 cutoff). API format well-known but verify field names. |
| Stack - MCP Client (pmcp) | MEDIUM | Existence and API from PROJECT.md. Not verified on crates.io. Fallback: rmcp. |
| Stack - AWS SDK additions | HIGH | Standard AWS SDK for Rust patterns. Version alignment straightforward. |
| Features - Table stakes | HIGH | Directly derived from PROJECT.md requirements and SDK capabilities |
| Features - Differentiators | HIGH | Based on direct SDK source analysis of ctx.map(), ctx.step() |
| Features - Anti-features | HIGH | Directly from PROJECT.md "Out of Scope" and sound engineering judgment |
| Architecture - Handler structure | HIGH | Combines verified SDK patterns (from examples/) with standard agent loop |
| Architecture - Replay safety | HIGH | Verified from replay.rs, execute.rs, execution_context.rs source |
| Architecture - MCP integration | MEDIUM | pmcp API from PROJECT.md, not verified from source. Connect-per-use is sound. |
| Pitfalls - Determinism | HIGH | Directly from SDK operation counter and replay mechanism source code |
| Pitfalls - Checkpoint size | HIGH | 750KB/256KB limits verified in source; budget math from typical payloads |
| Pitfalls - MCP connection | MEDIUM | MCP stateful protocol from training data; checkpoint constraint from SDK source |
| Pitfalls - API specifics | MEDIUM | Anthropic error codes, message pairing rules from training data |

**Overall confidence:** MEDIUM -- the core architecture and SDK integration patterns are high confidence (verified from source), but external dependencies (pmcp availability, Anthropic API type accuracy, AWS SDK version alignment) need validation before implementation begins.

### Gaps to Address

- **pmcp crate availability:** Is pmcp v2.0 published on crates.io or only available as a local/git dependency? Must be resolved before Phase 2. If unavailable, evaluate rmcp as alternative.
- **Anthropic API type accuracy:** ContentBlock variants, stop reason values, tool_use/tool_result exact field names should be verified against current API docs before Phase 1 implementation. Do not rely solely on training data.
- **AWS SDK version alignment:** Exact compatible versions of aws-sdk-dynamodb and aws-sdk-secretsmanager need checking against crates.io for the same SDK release train as aws-sdk-lambda ~1.112.0.
- **Integration testing strategy:** No local testing framework exists for durable execution replay. Testing approach needs definition -- likely unit tests with mock context (testutils feature) plus integration tests against deployed Lambda.
- **Durable execution timeout tuning:** Agent loops can run 2-10 minutes. The ExecutionTimeout in the SAM template needs appropriate configuration. Current examples use 300 seconds which may or may not be sufficient.
- **Child context strategy for iteration isolation:** Research suggests `run_in_child_context` per loop iteration would collapse completed iterations during replay, but this pattern is not demonstrated in existing examples. Needs validation during Phase 3.

## Sources

### Primary (HIGH confidence)
- SDK source code: `src/context/execution_context.rs`, `src/checkpoint/manager.rs`, `src/context/durable_context/mod.rs`, `src/context/durable_context/step/`, `src/context/durable_context/map/` -- operation ID generation, checkpoint limits, replay mechanism, map child contexts
- SDK examples: `examples/src/bin/map_operations/`, `examples/src/bin/parallel/`, `examples/template.yaml` -- deployment patterns, step/map usage
- Project config: `Cargo.toml`, `Cargo.lock`, `ARCHITECTURE.md` -- dependency versions, module structure
- Project planning: `.planning/PROJECT.md` -- requirements, constraints, existing assets

### Secondary (MEDIUM confidence)
- Anthropic Messages API format -- training data (stable, well-known API, but verify exact field names)
- MCP protocol tool schema format -- training data (JSON Schema-based inputSchema is core MCP spec)
- pmcp crate API (Client, HttpTransport, list_tools, call_tool) -- PROJECT.md description, not verified from source
- Agent orchestration patterns -- training data synthesis from LangChain, LangGraph, CrewAI, AutoGen, Claude agent patterns

### Tertiary (LOW confidence)
- pmcp v2.0 crate availability on crates.io -- claimed in PROJECT.md, needs verification
- rmcp (official MCP Rust SDK) existence and API -- from training data, needs verification
- AWS SDK exact version numbers for dynamodb/secretsmanager -- estimated from aws-sdk-lambda version

---
*Research completed: 2026-03-23*
*Ready for roadmap: yes*
