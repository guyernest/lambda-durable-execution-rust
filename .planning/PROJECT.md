# Durable Lambda MCP Agent

## What This Is

A Rust-based Durable Lambda agent that replaces the existing Step Functions agent pattern. The agent acts as an MCP client, connecting to MCP servers for tool discovery and execution, while using the existing Rust LLM caller for multi-provider LLM support. Managed via the existing AgentRegistry and admin UI alongside Step Functions agents.

## Core Value

A single Durable Lambda replaces the entire Step Functions orchestration — the agent loop is plain Rust code with checkpointed LLM calls and MCP tool executions, no state machine definition required.

## Requirements

### Validated

- ✓ Multi-provider LLM client with typed request/response, error classification, and Secrets Manager auth — Phase 1
- ✓ AgentRegistry config loading from DynamoDB with provider mapping and MCP server tool discovery with prefix routing — Phase 2

### Active

- [ ] Agent reads configuration from AgentRegistry DynamoDB table (instructions, LLM provider/model, MCP server endpoints, parameters)
- [ ] Agent connects to configured MCP servers via HTTP transport and discovers tools via `list_tools()`
- [ ] Agent calls LLM (Anthropic Claude) with message history and discovered MCP tool schemas
- [ ] Agent executes tool calls returned by LLM via MCP `call_tool()`, with parallel execution via `ctx.map()`
- [ ] Agent loop continues (LLM call → tool execution → append results → LLM call) until LLM returns final response
- [ ] Each LLM call and tool call is a durable `step()` operation — replayed from cache on Lambda resume
- [ ] Agent handles the Anthropic message format (assistant content blocks with tool_use, user content blocks with tool_result)
- [ ] Agent configuration includes list of MCP server endpoints replacing the DynamoDB Tool Registry
- [ ] Agent supports configurable retry for transient LLM and MCP failures
- [ ] Agent returns final LLM response as the durable execution result
- [ ] Agent is deployable via SAM template with DurableConfig, matching existing example patterns
- [ ] MCP tool schemas are translated to Claude API tool format for LLM calls

### Out of Scope

- Gemini and Bedrock/Nova transformers — the PoC includes Anthropic and OpenAI via the existing call_llm_rust code. Additional providers can be added later by porting the remaining transformers.
- Admin UI modifications — the AgentRegistry schema extension for MCP server endpoints is the interface; UI changes are a separate effort in the step-functions-agent repo.
- MCP server creation/wrapping — existing tools will be wrapped as MCP servers separately. This project builds the client/agent side.
- Streaming LLM responses — batch completion first, streaming can be added later.
- Human-in-the-loop via `wait_for_callback()` — valuable but not needed for the PoC agent loop.

## Context

### Existing Assets

- **Durable Execution SDK** (this repo): Provides `step()`, `map()`, `parallel()`, `wait()`, `wait_for_callback()` with checkpointing and replay. 332 tests, production-quality architecture.
- **Rust LLM Caller** (`~/projects/step-functions-agent/lambda/call_llm_rust`): UnifiedLLMService with provider transformers (Anthropic, OpenAI, Gemini, Bedrock), Secrets Manager integration, OpenTelemetry metrics. The Anthropic transformer and message models can be extracted/reused.
- **Rust MCP SDK** (`~/Development/mcp/sdk/rust-mcp-sdk`): `pmcp` v2.0.0 crate with full MCP client support — `Client`, `HttpTransport`, `list_tools()`, `call_tool()`, middleware (auth, retry, logging). HTTP/SSE transport suitable for Lambda-to-Lambda MCP communication.
- **AgentRegistry** (`~/projects/step-functions-agent/`): DynamoDB table with agent_name/version keys, system_prompt, llm_provider, llm_model, tools list, parameters (temperature, max_tokens, max_iterations). CDK-managed with GSIs.
- **Admin UI** (`~/projects/step-functions-agent/ui_amplify/`): Amplify-hosted management interface for viewing/editing agent configurations.

### Architecture Decision

The Step Functions agent routes tool calls through the state machine — each tool is a separate Lambda invoked by a Map state, with results flowing back through JSONata transformations. Adding MCP support to this architecture requires teaching Step Functions to speak MCP protocol, which is awkward because MCP is a stateful client-server protocol (connect → initialize → discover → call) that doesn't map well to stateless Step Functions task invocations.

The Durable Lambda approach makes MCP native: the agent IS an MCP client. Connection, discovery, and tool calls are all in-process. The durable execution framework handles the long-running nature (LLM calls, multiple tool iterations) via checkpointing.

### Runtime Constraint

AWS Durable Execution does not yet support the `provided.al2023` runtime. The Lambda must be configured with `nodejs24.x` runtime and `AWS_LAMBDA_EXEC_WRAPPER=/var/task/bootstrap` to run the Rust binary. This is the same pattern used by all examples in this repo and is a documented Lambda feature.

## Constraints

- **Tech stack**: Rust (edition 2021, MSRV 1.88), AWS Lambda with Durable Execution, SAM for deployment
- **Dependencies**: Must use `lambda-durable-execution-rust` (this crate), `pmcp` (MCP SDK), and reuse Anthropic-specific code from the existing Rust LLM caller
- **MCP transport**: HTTP/SSE for Lambda-to-MCP-server communication (stdio not viable in Lambda)
- **AgentRegistry compatibility**: Must read from the existing DynamoDB table schema; new fields (MCP server endpoints) should be additive, not breaking
- **Checkpoint limits**: 750KB per checkpoint batch — message histories for long conversations must stay within bounds or be managed

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Standardize on MCP for all tool interactions | Eliminates DynamoDB Tool Registry, simplifies discovery, enables any MCP-compatible tool server | -- Pending |
| Extract UnifiedLLMService from call_llm_rust | Reuse proven multi-provider abstraction (Anthropic + OpenAI for PoC) instead of rebuilding | -- Pending |
| Build as example binary in this repo | Keeps PoC close to the SDK it depends on; can extract to separate crate later | -- Pending |
| Extend AgentRegistry with mcp_servers field | Additive change to existing schema; existing Step Functions agents unaffected | -- Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd:transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-03-23 after Phase 2 completion*
