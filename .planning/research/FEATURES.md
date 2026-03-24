# Feature Landscape

**Domain:** AI Agent Platform Integration (Channels, Agent Teams, Platform UI, SDK Example)
**Researched:** 2026-03-24
**Confidence:** MEDIUM-HIGH (codebase analysis HIGH, ecosystem patterns MEDIUM, MCP Tasks spec HIGH from official docs)

## Context

This is a SUBSEQUENT MILESTONE feature map. The v1 durable agent (LLM client, MCP integration, agent loop, observability, deployment) is COMPLETE. This document covers the v2 integration features: channels, agent teams, pmcp-run Agents tab, and PMCP SDK example. Features reference existing working implementations in zeroclaw (channels), step-functions-agent (UI), and pmcp/pmcp-tasks (MCP Tasks).

## Table Stakes

Features that must exist for each capability area to be functional. Without these, the feature area is incomplete.

### Channels: Core Abstraction

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **Channel trait with send/receive** | Fundamental abstraction. zeroclaw proves this pattern with 12 implementations. Without a trait, every channel is bespoke code. Must support `send(message, recipient)` and async `listen(sender)`. | Low | None -- pure trait definition |
| **Channel registry (name-to-channel map)** | Agent needs to address channels by name ("slack", "discord", "approval"). zeroclaw uses `HashMap<String, Arc<dyn Channel>>` for this. | Low | Channel trait |
| **Slack channel implementation** | Most common enterprise communication channel. Step Functions agent already uses it for approvals. Direct REST API (chat.postMessage, conversations.history) -- no SDK needed. | Medium | Channel trait, Secrets Manager for bot token |
| **Webhook channel (inbound/outbound)** | Generic HTTP callback mechanism. Needed for: approval callbacks, external system notifications, inter-service communication. Maps naturally to `wait_for_callback()` -- the callback URL IS the webhook endpoint. | Medium | Channel trait, API Gateway or Lambda URL |
| **Durable channel send via ctx.step()** | Channel sends must be checkpointed so they are not re-sent on replay. A `ctx.step("send-slack-approval", ...)` ensures idempotent delivery. Without this, replays cause duplicate messages. | Low | Channel trait, existing durable primitives |
| **Durable channel receive via wait_for_callback()** | Receiving from a channel (approval response, task result, human input) maps directly to `wait_for_callback()`. The callback ID becomes the channel correlation ID. This is the key architectural insight -- channels are callbacks with names. | Medium | Channel trait, existing callback primitives |
| **Channel config in AgentRegistry** | Agent needs to know which channels are available and their credentials. Additive field `channels: [{type, name, config}]` in the existing DynamoDB schema. | Low | AgentConfig extension |
| **Deny-by-default security model** | zeroclaw enforces this: empty `allowed_users` means deny everyone. Critical for org-deployed agents where channels face external users. Autonomy levels (ReadOnly, Supervised, Full) per channel. | Medium | Channel trait, SecurityPolicy type |

### Channels: Approval Flow

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **Tool approval gate** | Core use case: agent encounters a "dangerous" tool (delete, payment, deploy), pauses, sends approval request via channel, waits for response. Without this, agents cannot be trusted with destructive operations. | Medium | Channel send/receive, tool tagging |
| **Tool danger classification** | Agent config must tag tools as requiring approval. Simple allowlist/denylist in AgentRegistry: `approval_required_tools: ["deploy__*", "db__delete"]`. Wildcard matching by prefix. | Low | AgentConfig extension |
| **Approval timeout with default-deny** | If no human responds within timeout, default to DENY. Prevents agents from blocking indefinitely. Uses `CallbackConfig::with_timeout()` already available in the SDK. | Low | wait_for_callback timeout (exists) |
| **Approval response with modification** | Approver should be able to modify tool arguments before approving (e.g., change target environment from prod to staging). Step Functions agent already has this in ApprovalDashboard.tsx (`modifiedInput` state). | Medium | Approval flow, channel message format |

### Agent Teams: Orchestration

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **Agent-as-MCP-tool wrapping** | Each team member agent is exposed as an MCP tool to the orchestrator. The tool's `inputSchema` is the agent's task description format. The tool's execution invokes the agent Lambda. This is the fundamental Agent Teams pattern. | High | Existing agent binary, Lambda invoke |
| **Dynamic MCP server generation** | Given a list of team member agents, generate an in-process MCP server (or tool list) that exposes each as a callable tool. No need for a real MCP server deployment -- just generate the tool definitions and route calls to Lambda invocations. | High | Agent-as-tool wrapping, MCP tool schema |
| **Sequential team execution via ctx.step()** | Orchestrator calls agents one at a time, passing context forward. Each agent call is a durable step. Natural fit for pipeline patterns (research -> draft -> review -> publish). | Medium | Agent-as-tool, ctx.step() |
| **Parallel team execution via ctx.parallel/map** | Orchestrator calls multiple agents simultaneously. Natural fit for fan-out patterns (analyze data from 5 different perspectives). Uses existing `ctx.map()` with agent tools. | Medium | Agent-as-tool, ctx.map() |
| **Shared context via MCP resources** | Team members need shared state (research findings, intermediate results). MCP resources are the right abstraction -- a shared resource server that team members can read/write. | High | MCP resource protocol, shared state store |
| **Model tiering for cost optimization** | Orchestrator uses a capable model (Opus) for planning and routing; worker agents use cheaper models (Sonnet, Haiku) for execution. Already supported by per-agent model config in AgentRegistry. | Low | AgentRegistry per-agent config (exists) |

### pmcp-run Agents Tab: CRUD

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **Agent list view** | Display all registered agents with name, status, model, MCP servers, last execution. Step Functions agent has this in Registries.tsx. Must use LCARS design system. | Medium | AgentRegistry API, LCARS components |
| **Agent create/edit form** | Form fields: name, instructions (system prompt), model selection, temperature, max_tokens, max_iterations, MCP server selection, channel config. Step Functions has AgentDetailsModal. | Medium | AgentRegistry write API, model registry |
| **Agent delete with confirmation** | Delete agent config from registry. Confirm dialog. Does not delete execution history (separate concern). | Low | AgentRegistry delete API |
| **Model registry/selector** | Dropdown of available models with provider, pricing, capabilities (tool support, vision). Step Functions has ModelCosts.tsx with full CRUD. | Medium | LLM Models DynamoDB table (migrate from Step Functions) |
| **MCP server selector** | Select from pmcp-run's existing registry of deployed MCP servers. Multi-select with endpoint URLs auto-populated. | Low | pmcp-run registry API (exists) |
| **API key management** | Manage API keys for LLM providers (Anthropic, OpenAI) via Secrets Manager. Step Functions has this in ModelCosts.tsx with PasswordField. | Medium | Secrets Manager API, per-provider key storage |

### pmcp-run Agents Tab: Execution

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **On-demand agent execution** | "Run Now" button with input textarea. Invokes the Durable Lambda with agent_name and user message. Step Functions has AgentExecution.tsx with agent selector and input. | Medium | Lambda invoke API, agent config |
| **Execution status tracking** | Show running/completed/failed status with real-time updates. Poll the durable execution checkpoint API or use DynamoDB streams. Step Functions uses Step Functions execution history API. | Medium | Execution tracking backend |
| **Execution history list** | Paginated list of past executions with status badges, duration, agent name, date. Step Functions has History.tsx with cursor-based pagination. | Medium | Execution history DynamoDB table |
| **Execution detail view** | View full conversation history (user messages, assistant responses, tool calls, tool results) for a completed execution. Step Functions has ExecutionDetail.tsx with MessageRenderer. | Medium | Execution history data, message rendering |

### PMCP SDK Example

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| **Durable agent as MCP server example** | The agent exposed as an MCP server tool with TaskSupport::Required. Client calls `tools/call` with `task` field, gets `CreateTaskResult`, polls `tasks/get`, retrieves result via `tasks/result`. Demonstrates the full MCP Tasks lifecycle. | High | pmcp-tasks crate, durable agent binary |
| **Task status mapping** | Map durable execution states to MCP Task states: running->working, completed->completed, failed->failed, waiting_for_callback->input_required. | Medium | MCP Tasks types, durable execution states |
| **Progress reporting** | Report iteration progress (iteration 3/10, tools called, tokens used) via MCP progress notifications during task execution. | Medium | MCP progress protocol, agent metadata |

## Differentiators

Features that set this platform apart. Not required for functionality but provide significant value.

### Channels: Advanced

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| **Discord channel** | Second most popular developer/community channel. WebSocket-based (Gateway API) for real-time messages. zeroclaw has full implementation with typing indicators and message splitting. | Medium | Channel trait, Discord bot token |
| **WhatsApp channel** | Enterprise messaging for non-technical approvers. Uses WhatsApp Business API (cloud-hosted). zeroclaw has implementation. | Medium | Channel trait, WhatsApp Business API setup |
| **Local agent channel** | Enables a local agent (zeroclaw running on developer's machine) to receive tasks from cloud agents and respond. The bridge between local and cloud agents. Uses webhook/SSE transport. | High | Channel trait, bidirectional transport |
| **Inter-agent channel** | Agent A sends a message to Agent B via named channel, not as a tool call. Useful for advisory patterns where one agent consults another without full orchestration overhead. | Medium | Channel trait, agent-to-agent routing |
| **Channel message routing with typing indicators** | Show "agent is thinking..." in Slack/Discord while the agent processes. zeroclaw has `start_typing`/`stop_typing` trait methods. Small but significant UX improvement. | Low | Channel trait (already has methods) |
| **Supervised channel listener with auto-restart** | zeroclaw's `spawn_supervised_listener` pattern: exponential backoff, health marking, restart counting. Critical for production channels that may disconnect. | Low | Channel trait, tokio spawned tasks |
| **Multi-channel broadcast** | Send the same message to multiple channels simultaneously (e.g., send approval request to both Slack and email). Simple fan-out over channel registry. | Low | Channel registry |

### Agent Teams: Advanced

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| **Hierarchical team topology** | Orchestrator decomposes task, assigns sub-tasks to specialized agents, synthesizes results. The proven enterprise pattern per industry research: planner uses capable model, workers use cheap models. | Medium | Agent-as-tool, sequential/parallel execution |
| **Pipeline pattern** | Agent output feeds directly into next agent's input. `ctx.step("agent-a") -> ctx.step("agent-b") -> ctx.step("agent-c")`. Natural for content workflows. | Low | Sequential execution (just code pattern) |
| **Agent handoff (explicit transfer)** | OpenAI Agents SDK pattern: agent decides which other agent should continue. Maps to a tool_use where the tool is another agent, and the loop continues in the new agent's context. | High | Agent-as-tool, context transfer |
| **Team-level cost budget** | Total cost limit across all team members. Orchestrator tracks cumulative token usage and stops when budget exceeded. Uses existing per-agent token tracking. | Medium | Token tracking (exists), budget config |
| **Consensus/voting pattern** | Multiple agents analyze the same input independently, orchestrator aggregates results. Good for accuracy-critical decisions. Uses `ctx.map()` for parallel execution. | Medium | Parallel execution, result aggregation logic |

### pmcp-run Agents Tab: Advanced

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| **Scheduled agent execution (cron)** | Run agents on schedule: "summarize Slack every morning at 9am", "check system health every hour". Uses EventBridge Scheduler to invoke Lambda. | Medium | EventBridge Scheduler, Lambda trigger |
| **Triggered agent execution (event-driven)** | Run agent when event occurs: new S3 object, DynamoDB stream, SNS notification. Already supported by Lambda event source mappings. | Medium | Lambda event source mappings |
| **Metrics dashboard with cost tracking** | Charts showing: cost over time (by model, by agent), token usage trends, execution counts, success rates, latency percentiles. Step Functions has Metrics.tsx with Recharts. | High | Metrics DynamoDB table, aggregation logic |
| **Approval dashboard** | Centralized view of pending approvals across all agents. Step Functions has ApprovalDashboard.tsx with polling. Replaces Activity-based polling with channel-based approach. | Medium | Channels (approval flow), pending approvals query |
| **Test prompt library** | Save and reuse test prompts per agent. Step Functions has this in AgentExecution.tsx (`TestPrompt` type with save/load). Speeds up agent development iteration. | Low | Test prompts DynamoDB table |
| **Agent version management** | Multiple versions per agent with promotion (draft -> active -> archived). DynamoDB sort key is already `version`. | Medium | AgentRegistry version field (exists), UI workflow |
| **Live execution streaming** | Show agent thinking in real-time: LLM responses streaming, tool calls executing. Requires WebSocket or SSE from Lambda to UI. | High | WebSocket API Gateway or polling, streaming support |

### PMCP SDK Example: Advanced

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| **Cancellation support** | Client sends `tasks/cancel`, durable execution receives cancellation signal, terminates gracefully. Maps to durable execution's termination manager. | Medium | MCP Tasks cancel, termination manager |
| **Input-required flow** | Task moves to `input_required` state when agent needs human input via channel. Client receives elicitation request. Maps durable `wait_for_callback` to MCP Tasks `input_required` status. | High | MCP Tasks input_required, channels |
| **Multi-tool task server** | MCP server exposing multiple agent configurations as separate tools. Each tool maps to a different agent in the registry. Single server, multiple agent capabilities. | Medium | pmcp server builder, AgentRegistry |

### Migration from Step Functions

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| **Execution history migration** | Import Step Functions execution history into the new DynamoDB format. Preserves operational continuity. One-time migration script. | Medium | Step Functions API, new history table |
| **Model costs migration** | Port the LLMModelsRegistry table data. Step Functions has full CRUD for model pricing. Copy table or write migration. | Low | DynamoDB table copy |
| **MCP server registry migration** | Port MCPServerRegistry data. Step Functions tracks server_id, endpoint_url, available_tools, health_check_url. pmcp-run already has a registry -- reconcile. | Medium | Two registry formats to merge |

## Anti-Features

Features to explicitly NOT build. Scope traps to avoid.

| Anti-Feature | Why Avoid | What to Do Instead |
|--------------|-----------|-------------------|
| **Slack Events API / Socket Mode listener in Lambda** | Lambda cannot maintain persistent WebSocket connections. Slack Socket Mode requires a long-running process. Lambda has a 15-minute max timeout. | Use webhook-based Slack integration (incoming webhooks for send, API calls for receive). For listening, use a separate lightweight service or poll conversations.history. Channels in Lambda are fire-and-forget sends + wait_for_callback receives. |
| **Real-time bidirectional streaming in Lambda** | Durable Lambda is request-response with checkpointing, not a streaming service. WebSocket state cannot be checkpointed. | Use polling from the UI. Agent writes execution state to DynamoDB; UI polls. MCP Tasks already defines a polling protocol with `pollInterval`. |
| **Channel listeners running inside Lambda** | Lambda is invocation-based. A "listener" pattern (long-polling Slack, maintaining WebSocket to Discord) does not fit. zeroclaw runs listeners because it is a long-running daemon. | Channels in Lambda are one-directional sends. For receiving, use `wait_for_callback()` with an external trigger (webhook, API Gateway, EventBridge). The channel abstraction in Lambda is simpler than zeroclaw's -- it is about formatted sends and structured receives, not background listeners. |
| **Full zeroclaw channel runtime port** | zeroclaw's channel runtime includes supervised listeners, memory context injection, script engines, per-message parallelism, auto-save memory. Most of this is irrelevant to a serverless agent. | Port the Channel trait (send + health_check, drop listen), the security model (deny-by-default, allowed_users), and the message format. The runtime supervision and listener management are local-agent concerns. |
| **Building a custom approval UI from scratch** | Step Functions already has ApprovalDashboard.tsx. pmcp-run has the LCARS design system. Building a third approval UI is wasteful. | Migrate the approval dashboard concept into pmcp-run's LCARS UI. Reuse the patterns (polling, task display, approve/deny/modify) but adapt to channel-based approvals instead of Step Functions Activity polling. |
| **Agent-to-agent communication via shared DynamoDB** | Tempting to have agents write to a shared DynamoDB table for coordination. Creates coupling, race conditions, and checkpoint size issues. | Use MCP resources for shared state. Each agent reads/writes through the MCP protocol, which provides clean abstraction boundaries. For direct communication, use channels. |
| **Streaming LLM responses to channels** | Sending partial LLM responses to Slack/Discord as they stream in creates message spam and poor UX. | Wait for complete LLM response, then send as a single message. This is how all production chat agents work. |
| **Custom orchestration DSL for agent teams** | Tempting to build a configuration language for team topologies (YAML workflow definitions). This recreates Step Functions in disguise. | Agent teams are plain Rust code using ctx.step/map/parallel. The orchestrator agent's system prompt defines the coordination pattern. No DSL needed -- the LLM IS the orchestrator. |
| **Per-message conversation persistence in channels** | Storing every channel message for cross-invocation memory. Adds storage complexity, privacy concerns, and checkpoint bloat. | Each agent invocation is stateless. Channel context is provided in the initial request. If memory is needed, use an MCP memory server tool. |
| **Multi-tenant channel isolation in single Lambda** | Trying to handle messages from multiple tenants/orgs in a single Lambda invocation. | One Lambda invocation = one agent execution. Multi-tenancy is at the config level (different AgentRegistry entries per tenant), not at the runtime level. |

## Feature Dependencies

```
Channels Abstraction:
  Channel trait
    -> Channel registry
    -> Slack implementation
    -> Webhook implementation
    -> Discord implementation (differentiator)
  Channel config in AgentRegistry
    -> Durable channel send (ctx.step)
    -> Durable channel receive (wait_for_callback)
      -> Tool approval gate
        -> Tool danger classification
        -> Approval timeout (default deny)
        -> Approval response with modification
  Deny-by-default security

Agent Teams:
  Agent-as-MCP-tool wrapping
    -> Dynamic MCP server generation
    -> Sequential execution (ctx.step)
    -> Parallel execution (ctx.map)
  Shared context (MCP resources) -- independent track
  Model tiering -- already exists in config

pmcp-run Agents Tab:
  Agent list view
    -> Agent create/edit form
    -> Agent delete
  Model registry (migrate from Step Functions)
    -> Model selector in agent form
  MCP server selector (use pmcp-run registry)
  On-demand execution
    -> Execution status tracking
    -> Execution history list
      -> Execution detail view
  API key management

PMCP SDK Example:
  Durable agent as MCP server
    -> Task status mapping
    -> Progress reporting
  (depends on: working agent binary, pmcp-tasks crate)

Migration:
  Model costs migration (independent, do first)
  Execution history migration (after new history format defined)
  MCP server registry migration (reconcile with pmcp-run)
```

Simplified critical path:
```
Channel trait + config -> Durable send/receive -> Approval flow
                                                  |
Agent-as-tool + team execution -----> Agent Teams
                                                  |
Agent CRUD UI + execution UI -------> Agents Tab
                                                  |
MCP Tasks integration --------------> SDK Example
```

## MVP Recommendation

### Phase 1: Channels Abstraction and Approval Flow
Build the channel trait, Slack and webhook implementations, durable send/receive, and tool approval gate. This is the highest-value feature because it unlocks human-in-the-loop for production use.

Prioritize:
1. Channel trait (simplified for Lambda -- send + health_check, no listener)
2. Channel config in AgentRegistry
3. Durable channel send via ctx.step()
4. Durable channel receive via wait_for_callback()
5. Webhook channel (simplest to implement, most versatile)
6. Slack channel (highest enterprise value)
7. Tool danger classification in config
8. Approval gate in agent loop

### Phase 2: pmcp-run Agents Tab (CRUD + Execution)
Port the management UI from Step Functions into pmcp-run. This gives the platform its control plane.

Prioritize:
1. Model costs migration (unblocks model selection)
2. Agent list view (LCARS design)
3. Agent create/edit form
4. On-demand execution
5. Execution history list
6. Execution detail view

### Phase 3: Agent Teams
Build agent-as-tool wrapping and team orchestration patterns. Depends on stable single-agent execution.

Prioritize:
1. Agent-as-MCP-tool wrapping
2. Sequential team execution
3. Parallel team execution
4. Hierarchical orchestration example

### Phase 4: PMCP SDK Example
Build the reference example demonstrating MCP Tasks with the durable agent.

Prioritize:
1. Durable agent as MCP server with TaskSupport::Required
2. Task status mapping (durable state -> MCP Task state)
3. Progress reporting
4. Cancellation support

### Defer Indefinitely:
- **Discord, WhatsApp, Signal channels** -- implement only when there is a concrete user request
- **Live execution streaming** -- high complexity, low priority given polling works
- **Agent handoff** -- complex, uncertain value over explicit orchestration
- **Scheduled/triggered execution** -- can use raw EventBridge without UI initially

## Checkpoint Budget Analysis (v2 Concerns)

The v1 analysis showed ~225KB for a 10-iteration agent loop, well within the 750KB limit. v2 features introduce new checkpoint data:

- **Channel messages**: Approval requests/responses add ~2-5KB per approval event. Typical agent has 0-3 approvals per execution. Impact: +15KB max. Negligible.
- **Agent team orchestration**: Each team member agent result is checkpointed. A 5-member team with 10KB results each = 50KB. A 3-level hierarchy with fan-out could reach 150KB. **Needs monitoring but should fit within budget.**
- **Shared context (MCP resources)**: If stored in checkpoint, could be large. **Should NOT be checkpointed** -- use external storage (DynamoDB/S3) and reference by ID.

**Mitigation**: Truncate large tool results before checkpointing; shared context stored externally; team results summarized before checkpointing to orchestrator.

## Sources

- `/Users/guy/projects/LocalAgent/zeroclaw/src/channels/` -- Channel trait, 12 implementations, supervised listener pattern, security policy (direct codebase analysis, HIGH confidence)
- `/Users/guy/projects/step-functions-agent/ui_amplify/src/pages/` -- 14 UI pages: Registries, History, Metrics, ModelCosts, AgentExecution, ExecutionDetail, ApprovalDashboard, MCPServers, Settings, ToolSecrets, Test, ToolTest, MCPTest, Dashboard (direct codebase analysis, HIGH confidence)
- `/Users/guy/Development/mcp/sdk/pmcp-run/` -- LCARS design system, authenticated routes (dashboard, registry, servers, deployments, settings), Next.js 14 + Amplify Gen 2 (direct codebase analysis, HIGH confidence)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/crates/pmcp-tasks/` -- MCP Tasks implementation with InMemoryTaskStore, DynamoDB store, Redis store, TaskRouter, security config (direct codebase analysis, HIGH confidence)
- [MCP Tasks Specification (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks) -- Task lifecycle, status states, polling, cancellation, input_required flow (official spec, HIGH confidence)
- [MCP 2026 Roadmap](http://blog.modelcontextprotocol.io/posts/2026-mcp-roadmap/) -- Tasks shipped as experimental, lifecycle gaps being addressed (official blog, HIGH confidence)
- [Multi-agent orchestration patterns 2025-2026](https://www.chanl.ai/blog/multi-agent-orchestration-patterns-production-2026) -- Hierarchical, pipeline, fan-out patterns; model tiering reduces costs 40-60% (industry analysis, MEDIUM confidence)
- [Human-in-the-loop for AI agents](https://www.permit.io/blog/human-in-the-loop-for-ai-agents-best-practices-frameworks-use-cases-and-demo) -- Approval workflows, channel-based notifications, policy-driven oversight (industry analysis, MEDIUM confidence)
- [Agent dashboard best practices](https://github.com/builderz-labs/mission-control) -- Agent fleet management, task dispatch, cost tracking patterns (open source reference, MEDIUM confidence)
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/context/durable_context/callback.rs` -- wait_for_callback API, CallbackConfig with timeout, heartbeat timeout (direct codebase analysis, HIGH confidence)
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/src/bin/mcp_agent/` -- Current agent handler, types, config, handler structure (direct codebase analysis, HIGH confidence)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/docs/TASKS_WITH_POLLING.md` -- pmcp Tasks integration guide, TypedTool with TaskSupport, requestor-driven detection (direct documentation, HIGH confidence)
