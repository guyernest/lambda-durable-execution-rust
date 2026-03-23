# Roadmap: Durable Lambda MCP Agent

## Overview

Build a Rust Lambda binary that implements an LLM agent loop (call Claude/GPT, execute MCP tools, repeat) using the durable execution SDK for replay-safe checkpointing. The project progresses from foundation types and LLM client, through configuration and MCP integration, into the core agent loop, then adds observability, and finally deploys with end-to-end validation. Each phase delivers a coherent, testable capability that unblocks the next.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: LLM Client** - Anthropic and OpenAI client with typed request/response, error classification, and Secrets Manager auth
- [ ] **Phase 2: Configuration and MCP Integration** - AgentRegistry config loading and MCP server connection, tool discovery, and schema translation
- [ ] **Phase 3: Agent Loop** - Core durable agent loop wiring LLM calls and MCP tool execution with replay-safe message history
- [ ] **Phase 4: Observability** - Token tracking, iteration metadata, and structured per-step logging
- [ ] **Phase 5: Deployment and Validation** - SAM template, IAM permissions, and end-to-end validation with real MCP servers

## Phase Details

### Phase 1: LLM Client
**Goal**: Agent can call Anthropic and OpenAI LLM APIs with typed requests/responses, classify errors for retry, and retrieve API keys securely
**Depends on**: Nothing (first phase)
**Requirements**: LLM-01, LLM-02, LLM-03, LLM-04, LLM-05, LLM-06, LLM-07
**Success Criteria** (what must be TRUE):
  1. A test can construct an LLM invocation request, send it to the Anthropic Messages API, and receive a typed response with content blocks and stop reason
  2. A test can construct an LLM invocation request, send it to the OpenAI Chat Completions API, and receive a typed response with the same unified response types
  3. HTTP 429/529/503 errors from either provider are classified as retryable; 400/401 errors are classified as non-retryable
  4. API keys are retrieved from AWS Secrets Manager using provider config's secret_path and secret_key_name
  5. Function calls (tool_use for Anthropic, tool_calls for OpenAI) are extracted from the unified response type regardless of which provider produced them
**Plans**: TBD

Plans:
- [ ] 01-01: TBD
- [ ] 01-02: TBD

### Phase 2: Configuration and MCP Integration
**Goal**: Agent can load its configuration from DynamoDB and connect to MCP servers to discover and translate tool schemas
**Depends on**: Phase 1
**Requirements**: CONF-01, CONF-02, CONF-03, CONF-04, MCP-01, MCP-02, MCP-03, MCP-06
**Success Criteria** (what must be TRUE):
  1. Agent loads system_prompt, llm_model, temperature, max_tokens, max_iterations, and mcp_servers from AgentRegistry DynamoDB table by agent_name/version
  2. Config loading is wrapped in a durable `ctx.step()` so it is cached on replay
  3. Agent connects to each configured MCP server via pmcp HttpTransport and calls `list_tools()` to discover available tools
  4. Discovered MCP tool schemas are translated into Claude API tool format (name, description, input_schema) ready for LLM calls
  5. If any MCP server fails to connect at startup, the agent fails fast with a clear error before calling the LLM
**Plans**: TBD

Plans:
- [ ] 02-01: TBD
- [ ] 02-02: TBD

### Phase 3: Agent Loop
**Goal**: Agent executes the complete durable loop -- LLM call, tool execution, result assembly, repeat -- until the LLM returns a final response
**Depends on**: Phase 2
**Requirements**: LOOP-01, LOOP-02, LOOP-03, LOOP-04, LOOP-05, LOOP-06, LOOP-07, MCP-04, MCP-05
**Success Criteria** (what must be TRUE):
  1. Agent calls LLM with message history and tools, receives tool_use blocks, executes the tools via MCP `call_tool()`, appends results, and repeats until the LLM returns end_turn with a final text response
  2. Each LLM call is a durable `ctx.step()` with ExponentialBackoff retry, and tool calls within an iteration are executed via `ctx.map()` for parallel execution and replay isolation
  3. Each loop iteration uses `run_in_child_context` so operation ID counters are isolated and replay is deterministic across suspension/resumption
  4. MCP tool errors (isError: true) are passed back to the LLM as error tool_results rather than failing the agent -- the LLM decides how to recover
  5. When max_iterations from config is exceeded, the agent returns a graceful error response rather than looping indefinitely
**Plans**: TBD

Plans:
- [ ] 03-01: TBD
- [ ] 03-02: TBD

### Phase 4: Observability
**Goal**: Agent tracks and reports token usage, iteration metadata, and per-step structured logs for debugging and cost visibility
**Depends on**: Phase 3
**Requirements**: OBS-01, OBS-02, OBS-03
**Success Criteria** (what must be TRUE):
  1. Token usage (input_tokens, output_tokens) is tracked for each LLM call and accumulated totals are available at the end of the run
  2. The final agent response includes iteration metadata: iteration count, total tokens consumed, list of tools called, and elapsed time
  3. Each durable step emits structured log entries with meaningful names (e.g., "llm-call-1", "tool-weather-api") via the SDK logger
**Plans**: TBD

Plans:
- [ ] 04-01: TBD

### Phase 5: Deployment and Validation
**Goal**: Agent is deployed to AWS via SAM and validated end-to-end against a real MCP server
**Depends on**: Phase 4
**Requirements**: DEPL-01, DEPL-02, DEPL-03
**Success Criteria** (what must be TRUE):
  1. Agent is deployable via SAM template with DurableConfig using the nodejs24.x runtime + EXEC_WRAPPER pattern, matching existing example patterns in this repo
  2. IAM role grants least-privilege permissions for DynamoDB (AgentRegistry read), Secrets Manager (API key read), and Lambda checkpoint operations
  3. A deployed agent invoked with a real MCP server completes an end-to-end run: loads config, discovers tools, calls LLM, executes tools, and returns a final response
**Plans**: TBD

Plans:
- [ ] 05-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4 -> 5

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. LLM Client | 0/0 | Not started | - |
| 2. Configuration and MCP Integration | 0/0 | Not started | - |
| 3. Agent Loop | 0/0 | Not started | - |
| 4. Observability | 0/0 | Not started | - |
| 5. Deployment and Validation | 0/0 | Not started | - |
