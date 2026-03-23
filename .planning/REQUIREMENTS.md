# Requirements: Durable Lambda MCP Agent

**Defined:** 2026-03-23
**Core Value:** A single Durable Lambda replaces Step Functions orchestration -- the agent loop is plain Rust code with checkpointed LLM calls and MCP tool executions.

## v1 Requirements

Requirements for initial release. Each maps to roadmap phases.

### LLM Client (adapted from call_llm_rust)

- [ ] **LLM-01**: UnifiedLLMService extracted/adapted from existing call_llm_rust with provider-agnostic request/response types (LLMInvocation, LLMResponse, function_calls, metadata)
- [ ] **LLM-02**: Anthropic transformer (AnthropicTransformer) for Claude models -- request/response mapping including tool_use/tool_result content blocks
- [ ] **LLM-03**: OpenAI transformer (OpenAITransformer) for GPT models -- request/response mapping including function calling format
- [ ] **LLM-04**: Provider config from AgentRegistry (provider_id, model_id, endpoint, auth config) matching existing call_llm_rust ProviderConfig schema
- [ ] **LLM-05**: API key retrieval from AWS Secrets Manager using secret_path and secret_key_name from provider config
- [ ] **LLM-06**: LLM error classification -- retryable (429, 529, 503) vs non-retryable (400, 401) mapped to durable step retry patterns
- [ ] **LLM-07**: Unified function_calls extraction from LLM response regardless of provider (tool_use blocks for Anthropic, tool_calls for OpenAI)

### Configuration

- [ ] **CONF-01**: Agent reads configuration from AgentRegistry DynamoDB table by agent_name and version
- [ ] **CONF-02**: Configuration includes system_prompt, llm_model, temperature, max_tokens, max_iterations
- [ ] **CONF-03**: Configuration includes mcp_servers array with endpoint URLs (additive field to existing schema)
- [ ] **CONF-04**: Config loading is a durable `ctx.step()` -- cached on replay

### MCP Integration

- [ ] **MCP-01**: Agent connects to configured MCP servers via pmcp HttpTransport and initializes each connection
- [ ] **MCP-02**: Agent discovers tools from each MCP server via `list_tools()` and merges into a unified tool list
- [ ] **MCP-03**: MCP tool schemas translated to Claude API tool format (name, description, input_schema)
- [ ] **MCP-04**: Agent executes tool calls via MCP `call_tool()` with tool results mapped to Anthropic tool_result content blocks
- [ ] **MCP-05**: MCP tool errors (isError: true) passed to LLM as error tool_results -- agent does not fail, LLM decides recovery
- [ ] **MCP-06**: MCP connection failure at startup fails fast with clear error (no calling LLM with zero tools)

### Agent Loop

- [ ] **LOOP-01**: Agentic loop: call LLM -> check for tool_use -> execute tools -> append results -> repeat until end_turn
- [ ] **LOOP-02**: Each LLM call is a durable `ctx.step()` with ExponentialBackoff retry for transient failures
- [ ] **LOOP-03**: Tool calls executed in parallel via `ctx.map()` when LLM returns multiple tool_use blocks
- [ ] **LOOP-04**: Each loop iteration uses `run_in_child_context` to isolate operation ID counters for replay determinism
- [ ] **LOOP-05**: Message history assembled incrementally from step results -- rebuilds naturally during replay
- [ ] **LOOP-06**: Max iterations guard from AgentRegistry config -- returns graceful error when exceeded
- [ ] **LOOP-07**: Final LLM response returned as durable execution result

### Observability

- [ ] **OBS-01**: Token usage (input_tokens, output_tokens) tracked per LLM call and accumulated across iterations
- [ ] **OBS-02**: Iteration metadata in final response: iteration count, total tokens, tools called, time elapsed
- [ ] **OBS-03**: Per-step structured logging via SDK logger with meaningful step names (e.g., "llm-call-1", "tool-weather-api")

### Deployment

- [ ] **DEPL-01**: Agent deployable via SAM template with DurableConfig (nodejs24.x runtime + EXEC_WRAPPER pattern)
- [ ] **DEPL-02**: IAM permissions for DynamoDB (AgentRegistry read), Secrets Manager (API key read), Lambda (checkpoint)
- [ ] **DEPL-03**: End-to-end validation with at least one real MCP server

## v2 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Context Window Management

- **CTX-01**: Context window overflow detection -- track cumulative tokens against model limits
- **CTX-02**: Conversation summarization on overflow -- extra LLM call to compress early history
- **CTX-03**: Message truncation strategy preserving tool_use/tool_result pairing integrity

### Safety & Validation

- **SAFE-01**: Tool call argument validation against MCP schema before sending to server
- **SAFE-02**: Sensitive tool confirmation via `wait_for_callback()` for destructive operations

### Additional Providers

- **PROV-01**: Gemini transformer support
- **PROV-02**: Bedrock/Nova transformer support

### Advanced Features

- **ADV-01**: Streaming LLM responses
- **ADV-02**: Non-text MCP tool results (images, resources)
- **ADV-03**: Agent-to-agent delegation via `ctx.invoke()`
- **ADV-04**: Prompt caching optimization for Anthropic API

## Out of Scope

| Feature | Reason |
|---------|--------|
| MCP server creation/wrapping | Separate effort -- this project builds the client/agent side only |
| Admin UI modifications | AgentRegistry schema extension is the interface; UI changes happen in step-functions-agent repo |
| Conversation persistence across invocations | Each invocation is stateless; long-term memory via MCP memory tool if needed |
| Dynamic tool loading mid-conversation | Breaks durable execution determinism -- tool set fixed per execution |
| Fine-grained cost budgets | Use max_iterations as cost proxy; token counting provides visibility |
| Custom tool result transformations | Pass MCP results as-is; truncation is the MCP server's responsibility |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| LLM-01 | Phase 1 | Pending |
| LLM-02 | Phase 1 | Pending |
| LLM-03 | Phase 1 | Pending |
| LLM-04 | Phase 1 | Pending |
| LLM-05 | Phase 1 | Pending |
| LLM-06 | Phase 1 | Pending |
| LLM-07 | Phase 1 | Pending |
| CONF-01 | Phase 2 | Pending |
| CONF-02 | Phase 2 | Pending |
| CONF-03 | Phase 2 | Pending |
| CONF-04 | Phase 2 | Pending |
| MCP-01 | Phase 2 | Pending |
| MCP-02 | Phase 2 | Pending |
| MCP-03 | Phase 2 | Pending |
| MCP-04 | Phase 3 | Pending |
| MCP-05 | Phase 3 | Pending |
| MCP-06 | Phase 2 | Pending |
| LOOP-01 | Phase 3 | Pending |
| LOOP-02 | Phase 3 | Pending |
| LOOP-03 | Phase 3 | Pending |
| LOOP-04 | Phase 3 | Pending |
| LOOP-05 | Phase 3 | Pending |
| LOOP-06 | Phase 3 | Pending |
| LOOP-07 | Phase 3 | Pending |
| OBS-01 | Phase 4 | Pending |
| OBS-02 | Phase 4 | Pending |
| OBS-03 | Phase 4 | Pending |
| DEPL-01 | Phase 5 | Pending |
| DEPL-02 | Phase 5 | Pending |
| DEPL-03 | Phase 5 | Pending |

**Coverage:**
- v1 requirements: 30 total
- Mapped to phases: 30
- Unmapped: 0

---
*Requirements defined: 2026-03-23*
*Last updated: 2026-03-23 after roadmap creation (LOOP-03 moved from Phase 4 to Phase 3)*
