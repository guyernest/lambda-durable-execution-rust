# Feature Landscape

**Domain:** AI Agent Orchestration (Durable Lambda MCP Agent)
**Researched:** 2026-03-23
**Confidence:** MEDIUM (training data for ecosystem patterns, HIGH for SDK constraints from codebase)

## Table Stakes

Features users expect. Missing = agent is non-functional or unreliable.

### Agent Loop Core

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| **Agentic loop (LLM call -> tool calls -> repeat)** | Fundamental agent pattern. Without this there is no agent. | Medium | Each LLM call is a `ctx.step()`. Loop continues until LLM returns `end_turn` / no tool_use blocks. Must handle Anthropic's content block array format (text + tool_use interleaved). |
| **Max iterations guard** | Prevents infinite loops and runaway costs. Every production agent system has this. | Low | Simple counter per loop. AgentRegistry already has `max_iterations` field. Return error or graceful summary when exceeded. |
| **Tool schema translation (MCP -> Claude API)** | Agent cannot call tools without correct schema format. MCP tool schemas use JSON Schema; Claude API expects `input_schema` on tool definitions. | Medium | MCP `Tool` has `name`, `description`, `inputSchema`. Claude API tool format needs `name`, `description`, `input_schema`. Mostly structural mapping, but edge cases around JSON Schema features (anyOf, $ref) need testing. |
| **Tool result formatting** | LLM needs tool results in the expected message format to continue reasoning. | Low | MCP `call_tool()` returns `CallToolResult` with `content` (text/image/resource). Must map to Anthropic `tool_result` content blocks. Text content is straightforward; image/resource content needs a serialization strategy. |
| **MCP server connection and tool discovery** | Agent must connect to configured MCP servers and discover available tools before first LLM call. | Medium | Use `pmcp` Client with HttpTransport. Connect -> initialize -> list_tools() for each configured server. Must handle multiple servers (merge tool lists, handle name collisions). Connection is NOT a durable step -- it's ephemeral per Lambda invocation since MCP connections are stateful and cannot be serialized. |
| **AgentRegistry config loading** | Agent must read its configuration (instructions, model, MCP endpoints, parameters) from DynamoDB. | Low | Single DynamoDB GetItem. Straightforward with AWS SDK for Rust. Should be a `ctx.step()` so config is cached on replay. |
| **LLM API call with retry** | Anthropic API calls can fail transiently (429 rate limits, 529 overloaded). Must retry. | Low | Use `ctx.step()` with `ExponentialBackoff` retry strategy. The durable SDK handles retry logic already. Map Anthropic HTTP errors to retryable vs non-retryable. |
| **Message history assembly** | Each LLM call needs the full conversation history (system prompt + user messages + assistant responses + tool results). | Medium | Must accumulate messages across loop iterations. The message array is the core state of the agent. Each entry is a `ctx.step()` result that gets replayed, so message history rebuilds naturally during replay. |
| **Deterministic handler design** | Durable execution replays the handler from scratch. Non-deterministic code (random, timestamps, network calls outside `step()`) breaks replay. | Low | Architecture constraint, not a feature to build. All side effects inside `ctx.step()`. MCP connections are re-established each invocation (not checkpointed). Random values for jitter etc. belong inside steps. |

### Error Handling

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| **LLM error classification** | Must distinguish retryable (rate limit, server error) from non-retryable (invalid request, auth failure) errors to use SDK retry correctly. | Low | Anthropic returns HTTP status codes: 429 = rate limited (retryable), 529 = overloaded (retryable), 400 = bad request (not retryable), 401 = auth (not retryable). Map to step error patterns. |
| **MCP tool call error handling** | Tools can fail. LLM expects `tool_result` with `is_error: true` so it can reason about the failure and try alternatives. | Low | MCP `call_tool()` returns `CallToolResult` with `isError` flag. Pass error content back to LLM as tool_result with error flag. Do NOT fail the agent -- let the LLM decide how to recover. |
| **Graceful agent failure** | If the agent truly cannot continue (all retries exhausted, non-recoverable error), it should return a structured error, not crash opaquely. | Low | Catch `DurableError` variants, return structured JSON with error classification, last known state, and iteration count. |
| **MCP connection failure handling** | MCP servers may be down or unreachable. Agent should fail early with clear error rather than calling LLM with zero tools. | Low | Validate that at least one MCP server connected successfully and at least one tool was discovered. Fail fast with clear error message otherwise. |

### Configuration

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| **System prompt from AgentRegistry** | The system prompt defines agent behavior. Must be configurable per agent, not hardcoded. | Low | Read `system_prompt` / `instructions` field from AgentRegistry. Pass as `system` parameter in Anthropic API call. |
| **Model selection from AgentRegistry** | Different agents need different models (Sonnet for fast/cheap, Opus for complex reasoning). | Low | Read `llm_model` field. Pass to Anthropic API. Initially just claude-sonnet-4-20250514 and claude-opus-4-20250514. |
| **Temperature and max_tokens from config** | Standard LLM parameters that affect agent behavior. Must be configurable. | Low | Read from AgentRegistry `parameters` map. Pass to Anthropic API request. |
| **MCP server endpoints from config** | Agent must know which MCP servers to connect to. This is the replacement for the DynamoDB Tool Registry. | Low | New field in AgentRegistry: `mcp_servers` array of `{url, name?, auth?}` objects. Additive schema change. |

## Differentiators

Features that set this apart from the Step Functions agent pattern. Not expected for PoC, but highly valuable.

### Durable Execution Advantages

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| **Parallel tool execution via `ctx.map()`** | When LLM returns multiple tool_use blocks in a single response, execute them concurrently. Step Functions does this with a Map state, but here it is one line of code: `ctx.map(tool_calls, execute_tool, config)`. Significantly faster for multi-tool turns. | Low | Natural fit -- LLM returns Vec<ToolUse>, map over them. Each tool call is a durable child operation. Concurrency bounded by `MapConfig::with_max_concurrency()`. If any tool call fails, others still complete (graceful degradation). |
| **Checkpoint-based conversation durability** | If Lambda is suspended mid-conversation (timeout, memory, checkpoint), it resumes exactly where it left off. No conversation state lost. Step Functions achieves this with explicit state passing between states; durable execution gets it for free. | Free | Already provided by the SDK. Each `ctx.step()` result is cached. On replay, all previous LLM calls and tool results are returned from cache. No explicit state management code needed. |
| **Cost efficiency via suspension** | Lambda suspends (no compute cost) during `ctx.wait()` or between retries. Step Functions charges per state transition. A 10-iteration agent loop with retries could cost significantly less. | Free | Inherent to durable execution. Wait periods and retry backoffs use `ctx.wait()` which suspends the Lambda. |
| **Single-function deployment** | One Lambda function, one SAM resource. Step Functions agents require: state machine definition, multiple Lambda functions (router, tool executor, response formatter), IAM roles for each. | Free | Architecture advantage. One `sam deploy` vs complex CloudFormation. |
| **Structured iteration logging** | Each durable step gets a name and is tracked in the checkpoint history. The execution trace IS the agent's reasoning log. No separate logging infrastructure needed for observability. | Low | Name each step meaningfully: `"llm-call-1"`, `"tool-weather-api"`, `"llm-call-2"`. The checkpoint history becomes a structured trace. Can be queried via AWS APIs. |

### Context Window Management

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| **Token counting per LLM call** | Track input/output tokens from Anthropic `usage` response field. Essential for cost tracking and context window awareness. | Low | Anthropic returns `usage: { input_tokens, output_tokens }` on every response. Store in agent state. Accumulate across iterations. Include in final response metadata. |
| **Context window overflow detection** | Detect when message history approaches model context limit before the LLM call fails with a 400 error. | Medium | Track cumulative tokens. Compare against model limits (200K for Sonnet/Opus). When approaching limit (~90%), agent can: (a) return what it has, (b) summarize conversation, or (c) truncate early messages. For PoC, option (a) is sufficient. |
| **Conversation summarization on overflow** | When context window fills up, summarize early conversation history to reclaim token budget. This is what makes long-running agents viable. | High | Requires an extra LLM call to summarize, then replace early messages with summary. Complex because: must preserve tool call/result pairing integrity, system prompt must stay, recent messages more important than old ones. Defer past PoC. |

### Observability

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| **Iteration metadata in response** | Return structured metadata alongside the final answer: iteration count, total tokens used, tools called, time elapsed. Makes agent behavior transparent. | Low | Accumulate metrics during agent loop. Return as part of the durable execution result alongside the LLM's final text response. |
| **Per-step structured logging** | Use the SDK's `step_ctx.info()` / `step_ctx.warn()` for structured logs within each durable step. Logs are tied to specific operations. | Low | SDK already provides `DurableLogData` with operation context. Use consistently for LLM calls ("Calling claude-sonnet-4-20250514, 3847 input tokens"), tool calls ("Executing tool weather-api"), and decisions ("Max iterations reached"). |

### Safety

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| **Tool call validation** | Validate tool call arguments against schema before sending to MCP server. Catch malformed LLM outputs before they cause MCP errors. | Medium | JSON Schema validation of tool arguments against the schema returned by `list_tools()`. Prevents garbage-in to tool servers. If validation fails, return error to LLM as tool_result so it can self-correct. |
| **Sensitive tool confirmation (future)** | Some tools (delete operations, financial transactions) should require human confirmation. Durable execution's `wait_for_callback()` is purpose-built for this. | Medium | Not needed for PoC, but the architecture supports it naturally. Tag certain tools as requiring confirmation in agent config. When encountered, use `ctx.wait_for_callback()` to pause for human approval. |

## Anti-Features

Features to explicitly NOT build in the PoC. Scope discipline.

| Anti-Feature | Why Avoid | What to Do Instead |
|--------------|-----------|-------------------|
| **Multi-provider LLM support** | The existing Rust LLM caller already handles OpenAI/Gemini/Bedrock. Abstracting providers in the PoC adds complexity without validating the core value proposition (durable MCP agent loop). | Build for Anthropic Claude only. Structure code so provider abstraction can be added later (separate transformer module) but do not implement it now. |
| **Streaming LLM responses** | Streaming adds significant complexity (partial content blocks, incremental tool_use parsing, checkpoint timing). The agent loop needs complete responses to make decisions. | Use batch completion (`/v1/messages` without streaming). Streaming can be added as an optimization later for user-facing scenarios. |
| **Human-in-the-loop approval flows** | `wait_for_callback()` is available in the SDK, but building the approval UI, webhook infrastructure, and timeout handling is a separate project. | Acknowledge in architecture docs that this is possible. Do not build it. The SDK's callback support means it can be added without architectural changes. |
| **Conversation persistence / memory across invocations** | Long-term memory (remembering past conversations) requires a separate storage layer and retrieval mechanism. Not needed to validate the core agent loop. | Each agent invocation is stateless across invocations. Conversation history lives only within a single durable execution. If needed later, tool-based memory (MCP server with a memory tool) is the right pattern. |
| **Agent-to-agent delegation** | Multi-agent systems (one agent delegating to another) are fashionable but add enormous complexity. The SDK's `ctx.invoke()` could enable this, but it is premature. | Single agent per invocation. If multi-agent is needed, each agent is a separate Lambda; orchestration happens at a higher level, not within the agent itself. |
| **Dynamic tool loading / hot reload** | Reconnecting to MCP servers mid-conversation to pick up new tools. Breaks determinism requirements of durable execution (replay would see different tool sets). | Tool discovery happens once at agent start, before the agentic loop. Tool set is fixed for the duration of one execution. |
| **Fine-grained cost budgets** | Per-invocation cost limits ("stop after $5 of API calls"). Requires token-to-cost mapping, model pricing tables, real-time tracking. | Use max_iterations as the cost proxy. Token counting provides visibility but not enforcement. Cost budgets can be added later using the token tracking data. |
| **Prompt caching optimization** | Anthropic supports prompt caching (cache system prompt + early messages to reduce input tokens on subsequent calls). Useful but an optimization, not a core feature. | Structure messages so prompt caching CAN work (system prompt is stable, tool definitions are stable). Do not implement cache control headers or cache hit tracking in PoC. |
| **Image/file content in tool results** | MCP tools can return image and resource content types. Full support requires base64 encoding, content type negotiation, and LLM multimodal input formatting. | Support text content from MCP tool results only. Log a warning for non-text content. Can be extended later. |
| **Custom tool result transformations** | Post-processing tool outputs before feeding to LLM (truncation, formatting, extraction). | Pass MCP tool results to LLM as-is (text content). If tool results are too large, that is the MCP server's problem to fix, not the agent's. |

## Feature Dependencies

```
AgentRegistry config loading
  -> MCP server connection and tool discovery (needs endpoint URLs from config)
  -> Tool schema translation (needs MCP tool list)
  -> Agent loop core (needs translated tools + system prompt)
    -> LLM API call with retry (called each iteration)
    -> Message history assembly (accumulates across iterations)
    -> Tool result formatting (after each tool call)
    -> Parallel tool execution (when multiple tool_use blocks returned)
    -> Max iterations guard (checked each iteration)
    -> Token counting (after each LLM response)

MCP tool call error handling -> Tool result formatting (errors are a type of result)
LLM error classification -> LLM API call with retry (determines retry behavior)
Context window overflow detection -> Token counting (needs cumulative token data)
```

Simplified critical path:
```
Config -> MCP Connect -> Schema Translate -> [LLM Call -> Tool Execute -> Format Result] loop -> Return
```

## MVP Recommendation

### Phase 1: Core Agent Loop (must ship first)

Build in this order, each building on the previous:

1. **Anthropic message types and client** -- foundational types everything else depends on
2. **AgentRegistry config loading** -- unblocks everything else
3. **MCP server connection and tool discovery** -- agent needs tools to be useful
4. **Tool schema translation (MCP -> Claude API)** -- LLM needs tools in its format
5. **Agent loop with durable steps** -- ties LLM calls and tool execution together
6. **Max iterations guard** -- safety net

### Phase 2: Production Hardening

7. **Parallel tool execution via `ctx.map()`** -- performance win, low complexity
8. **MCP tool call error handling** -- let LLM reason about failures
9. **LLM error classification** -- better retry behavior
10. **Graceful agent failure** -- structured error responses
11. **Token counting per LLM call** -- visibility into costs

### Defer to Later Phases

- **Context window overflow detection** -- needs token counting first, medium complexity
- **Conversation summarization on overflow** -- high complexity, not needed for reasonable conversation lengths
- **Tool call validation** -- nice to have, LLM outputs are usually well-formed for Claude
- **Iteration metadata in response** -- easy but not blocking

## Checkpoint Budget Analysis

**Critical constraint:** 750KB per checkpoint batch.

Each `ctx.step()` checkpoints its result. For the agent loop, the checkpointed data includes:
- Agent config (~1-5KB): One-time, small
- Each LLM response (~2-30KB): Varies by response length. A typical Claude response is 500-2000 tokens = 2-8KB JSON. Tool-heavy responses with long outputs can be 20-30KB.
- Each tool call result (~1-20KB): Varies wildly by tool. Simple tools return < 1KB. Database queries or web scrapes could return 10-20KB.

**Budget math for a 10-iteration agent loop:**
- Config: 5KB
- 10 LLM calls: 10 x 10KB = 100KB
- 20 tool calls (2 per iteration avg): 20 x 5KB = 100KB
- Overhead (names, metadata): ~20KB
- **Total: ~225KB** -- well within 750KB limit

**When it becomes a problem:**
- 20+ iterations with verbose tool results
- Tools returning large payloads (full web pages, large query results)
- LLM generating very long responses

**Mitigation (not for PoC):** Custom `Serdes` on step configs to compress large payloads; truncation of tool results before checkpointing; context window summarization reduces both token cost AND checkpoint size.

## Sources

- Durable Execution SDK codebase: checkpoint limit (750KB), step/map/parallel APIs, retry presets, error types -- directly read from source
- PROJECT.md: AgentRegistry schema, existing assets, constraints, out-of-scope decisions
- Anthropic Messages API: tool_use / tool_result content block format, usage field for token counting, HTTP status codes for error classification -- training data (MEDIUM confidence, well-established API)
- MCP Protocol: Tool schema format, call_tool response structure, HTTP transport -- training data (MEDIUM confidence, protocol is stable post-1.0)
- Agent orchestration patterns: agentic loop, context window management, multi-tool execution, max iterations -- training data synthesis from LangChain, LangGraph, CrewAI, AutoGen, Claude agent patterns (MEDIUM confidence)
