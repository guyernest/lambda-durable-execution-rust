# Roadmap: Durable Lambda MCP Agent Platform

## Milestones

- **v1.0 Durable MCP Agent** - Phases 1-5 (shipped 2026-03-23)
- **v2.0 Integration Plan** - Phases 6-9 (in progress)

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

<details>
<summary>v1.0 Durable MCP Agent (Phases 1-5) - SHIPPED 2026-03-23</summary>

- [x] **Phase 1: LLM Client** - Anthropic and OpenAI client with typed request/response, error classification, and Secrets Manager auth
- [x] **Phase 2: Configuration and MCP Integration** - AgentRegistry config loading and MCP server connection, tool discovery, and schema translation
- [x] **Phase 3: Agent Loop** - Core durable agent loop wiring LLM calls and MCP tool execution with replay-safe message history
- [x] **Phase 4: Observability** - Token tracking, iteration metadata, and structured per-step logging
- [x] **Phase 5: Deployment and Validation** - SAM template, IAM permissions, and end-to-end validation with real MCP servers

</details>

### v2.0 Integration Plan (Phases 6-9)

- [ ] **Phase 6: PMCP SDK Example** - Reference MCP agent example demonstrating LLM + MCP tool loop with Durable Lambda, task-aware tool execution, and progress logging
- [ ] **Phase 7: pmcp-run Agents Tab** - Agent CRUD UI in LCARS design system, model registry, on-demand execution, execution history, metrics dashboard, and cost tracking
- [ ] **Phase 8: Channels and Approval Flow** - Channel abstraction with Slack/Discord/webhook adapters, webhook receiver Lambda, durable send/receive, deny-by-default security, and tool approval gates
- [ ] **Phase 9: Agent Teams** - Agent-as-MCP-tool wrapping, sequential and parallel team execution, delegation depth guard, circular delegation prevention, shared context via S3, and response summarization

## Phase Details

<details>
<summary>v1.0 Phase Details (Phases 1-5)</summary>

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
**Plans**: 3 plans

Plans:
- [x] 01-01-PLAN.md — Foundation types, error classification, and project scaffold (models.rs, error.rs, utils.rs, Cargo.toml)
- [x] 01-02-PLAN.md — Anthropic and OpenAI transformers with MessageTransformer trait and TransformerRegistry
- [x] 01-03-PLAN.md — SecretManager and UnifiedLLMService with complete LLM invocation pipeline

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
**Plans**: 2 plans

Plans:
- [x] 02-01-PLAN.md — AgentConfig types, DynamoDB loader, provider mapping, ConfigError (config/ module)
- [x] 02-02-PLAN.md — MCP client integration with tool discovery, schema translation, prefix routing (mcp/ module)

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
**Plans**: 2 plans

Plans:
- [x] 03-01-PLAN.md — Agent types (request/response/iteration), MCP tool execution, and durable handler with full agent loop
- [x] 03-02-PLAN.md — Unit tests for handler helpers and types, full suite verification and cleanup

### Phase 4: Observability
**Goal**: Agent tracks and reports token usage, iteration metadata, and per-step structured logs for debugging and cost visibility
**Depends on**: Phase 3
**Requirements**: OBS-01, OBS-02, OBS-03
**Success Criteria** (what must be TRUE):
  1. Token usage (input_tokens, output_tokens) is tracked for each LLM call and accumulated totals are available at the end of the run
  2. The final agent response includes iteration metadata: iteration count, total tokens consumed, list of tools called, and elapsed time
  3. Each durable step emits structured log entries with meaningful names (e.g., "llm-call-1", "tool-weather-api") via the SDK logger
**Plans**: 1 plan

Plans:
- [x] 04-01-PLAN.md — AgentMetadata type, token tracking, tool name collection, elapsed time, and structured logging in handler

### Phase 5: Deployment and Validation
**Goal**: Agent is deployed to AWS via SAM and validated end-to-end against a real MCP server
**Depends on**: Phase 4
**Requirements**: DEPL-01, DEPL-02, DEPL-03
**Success Criteria** (what must be TRUE):
  1. Agent is deployable via SAM template with DurableConfig using the nodejs24.x runtime + EXEC_WRAPPER pattern, matching existing example patterns in this repo
  2. IAM role grants least-privilege permissions for DynamoDB (AgentRegistry read), Secrets Manager (API key read), and Lambda checkpoint operations
  3. A deployed agent invoked with a real MCP server completes an end-to-end run: loads config, discovers tools, calls LLM, executes tools, and returns a final response
**Plans**: 1 plan

Plans:
- [x] 05-01-PLAN.md — SAM template (McpAgentFunction + AgentRegistryTable + IAM) and end-to-end validation script

</details>

### Phase 6: PMCP SDK Example
**Goal**: A reference MCP agent example in the PMCP SDK repo demonstrates how to build an LLM + MCP tool loop agent using Durable Lambda, with client-side MCP Tasks handling and progress logging
**Depends on**: Phase 5 (v1.0 complete)
**Requirements**: SDK-01, SDK-02, SDK-03
**Success Criteria** (what must be TRUE):
  1. A self-contained example in the PMCP SDK compiles and demonstrates a durable agent that connects to MCP servers, discovers tools, calls the LLM, executes tools, and loops until end_turn
  2. When a tool returns a Task (long-running), the agent polls via ctx.wait_for_condition() using tasks_get() until the task reaches a terminal status, with Lambda suspending between polls
  3. Agent execution emits structured tracing logs with iteration count, token usage, and tool names at each iteration
**Plans**: 2 plans

Plans:
- [x] 06-01-PLAN.md — Cargo.toml dev-dependencies and core agent example with LLM + MCP tool loop (SDK-01, SDK-03)
- [ ] 06-02-PLAN.md — Task-aware tool execution with wait_for_condition polling (SDK-02)

### Phase 7: pmcp-run Agents Tab
**Goal**: Users can create, configure, execute, and monitor agents through the pmcp-run web UI without touching DynamoDB, SAM templates, or the command line
**Depends on**: Phase 6
**Requirements**: PMCP-01, PMCP-02, PMCP-03, PMCP-04, PMCP-05, PMCP-06, PMCP-07, PMCP-08, PMCP-09, PMCP-10, PMCP-11, PMCP-12
**Success Criteria** (what must be TRUE):
  1. User navigates to the Agents tab and sees a list of all configured agents with their name, status, model, and connected MCP servers
  2. User creates a new agent through a form (instructions, model, MCP servers, channel config, parameters), and the agent appears in the list and is immediately invocable
  3. User triggers an agent execution from the UI with custom input, sees the execution status update from running to completed/failed, and can view the full conversation history including tool calls and results
  4. User views a metrics dashboard showing token usage charts and execution success rates across agents, with cost tracking broken down by model
  5. User manages LLM provider API keys through the UI via Secrets Manager integration, and selects from a model registry with provider and pricing information
**Plans**: TBD

### Phase 8: Channels and Approval Flow
**Goal**: Agent can send messages to and receive responses from external platforms (Slack, Discord, webhooks) through named channels, and can pause for human approval before executing dangerous tools
**Depends on**: Phase 7
**Requirements**: CHAN-01, CHAN-02, CHAN-03, CHAN-04, CHAN-05, CHAN-06, CHAN-07, CHAN-08, CHAN-09, CHAN-10, CHAN-11, APPR-01, APPR-02, APPR-03, APPR-04
**Success Criteria** (what must be TRUE):
  1. Agent sends a message to a configured Slack channel and the message appears in the correct Slack workspace/channel within seconds
  2. Agent sends an approval request for a dangerous tool call, suspends via wait_for_callback(), and resumes with the human's approve/deny/modify response when the webhook receiver processes the callback
  3. Webhook Receiver Lambda receives a Slack/Discord interaction payload, validates its signature, returns 200 within 3 seconds, and asynchronously resumes the suspended agent via SendDurableExecutionCallbackSuccess
  4. An agent configured with no allowed channels cannot send or receive through any channel, even if channel adapters are registered in the system
  5. Approval requests that exceed their configured timeout are automatically denied and the agent continues without executing the tool
**Plans**: TBD

### Phase 9: Agent Teams
**Goal**: An orchestrator agent can delegate work to specialist agents -- sequentially for pipeline patterns or in parallel for fan-out patterns -- with safeguards against circular delegation and checkpoint overflow
**Depends on**: Phase 8
**Requirements**: TEAM-01, TEAM-02, TEAM-03, TEAM-04, TEAM-05, TEAM-06, TEAM-07, TEAM-08, TEAM-09
**Success Criteria** (what must be TRUE):
  1. An orchestrator agent sees its team members as callable tools (with descriptions derived from each member's agent config) and can invoke them through the normal LLM tool-use flow
  2. A team of 3 specialists invoked via ctx.map() executes in parallel, and each specialist's response is summarized to a configurable size limit before being checkpointed to the orchestrator's context
  3. An agent delegation chain that exceeds the configured maximum depth (e.g., 3) is rejected with a clear error rather than creating additional invocations
  4. An agent that has already been visited in the current delegation chain cannot be delegated to again, preventing circular loops (A delegates to B delegates to A)
  5. Team members can read and write shared context files via S3 Files MCP resources during their execution
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 6 -> 7 -> 8 -> 9

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. LLM Client | v1.0 | 3/3 | Complete | 2026-03-23 |
| 2. Configuration and MCP Integration | v1.0 | 2/2 | Complete | 2026-03-23 |
| 3. Agent Loop | v1.0 | 2/2 | Complete | 2026-03-23 |
| 4. Observability | v1.0 | 1/1 | Complete | 2026-03-23 |
| 5. Deployment and Validation | v1.0 | 1/1 | Complete | 2026-03-23 |
| 6. PMCP SDK Example | v2.0 | 1/2 | In progress | - |
| 7. pmcp-run Agents Tab | v2.0 | 0/? | Not started | - |
| 8. Channels and Approval Flow | v2.0 | 0/? | Not started | - |
| 9. Agent Teams | v2.0 | 0/? | Not started | - |
