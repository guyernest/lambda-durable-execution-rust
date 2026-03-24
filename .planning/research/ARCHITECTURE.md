# Architecture Patterns

**Domain:** Durable Lambda MCP Agent Platform -- Channels, Agent Teams, pmcp-run Integration
**Researched:** 2026-03-24
**Milestone:** v2.0 Integration Plan
**Confidence:** HIGH for SDK integration points (verified from source), MEDIUM for cross-system data flow (inferred from multiple codebase reads), LOW for MCP Tasks integration (spec is experimental)

## Context: What Already Exists

Before describing new components, here is the system as-built after v1.0:

```
examples/src/bin/mcp_agent/
  main.rs              -- Lambda entry: init LLM service, with_durable_execution_service()
  handler.rs           -- agent_handler(): load config -> discover tools -> agent loop
  config/
    loader.rs          -- DynamoDB GetItem, parse_agent_config(), map_provider_config()
    types.rs           -- AgentConfig, AgentParameters
  llm/
    service.rs         -- UnifiedLLMService::process(LLMInvocation) -> LLMResponse
    models.rs          -- ProviderConfig, UnifiedMessage, ContentBlock, FunctionCall, etc.
    error.rs           -- LLMError enum
    transformers/      -- Anthropic and OpenAI request/response transformers
  mcp/
    client.rs          -- discover_all_tools(), establish_mcp_connections(), execute_tool_call()
    types.rs           -- ToolsWithRouting { tools, routing }
    error.rs           -- McpError enum
  types.rs             -- AgentRequest, AgentResponse, AgentMetadata, IterationResult, ToolCallResult
```

**SAM deployment:** `examples/template-agent.yaml` -- single Lambda with DurableConfig, AgentRegistry DynamoDB table.

**Durable SDK primitives used:** `ctx.step()` (config load, tool discovery, LLM calls), `ctx.map()` (parallel tool execution), `ctx.run_in_child_context()` (per-iteration isolation).

**Durable SDK primitives NOT yet used:** `ctx.wait_for_callback()`, `ctx.wait()`, `ctx.invoke()`, `ctx.parallel()`.

## Recommended Architecture: v2.0 Extension

The v2.0 milestone adds three major capabilities to the existing agent binary. The core architectural principle is: **new features compose with existing primitives rather than replacing them**. Channels wrap `wait_for_callback()`. Agent Teams wrap `ctx.invoke()` and MCP server generation. The pmcp-run Agents tab is a UI layer over the existing AgentRegistry + new DynamoDB fields.

### System Overview

```
+------------------+     +------------------+     +------------------+
|   pmcp-run UI    |     |  External APIs   |     | Agent Lambda B   |
|  (Next.js LCARS) |     | (Slack, Discord) |     | (Team member)    |
+--------+---------+     +--------+---------+     +--------+---------+
         |                         |                        |
    AppSync GraphQL          Webhook / API             ctx.invoke()
         |                         |                        |
+--------+---------+     +--------+---------+     +--------+---------+
| Agent Lambda A   |<--->| Channel Router   |<--->| Dynamic MCP Srvr |
| (Durable Agent)  |     | (in-handler)     |     | (agent-as-tool)  |
+--------+---------+     +------------------+     +------------------+
         |
   Checkpoint API
         |
+--------+---------+
| AWS Durable Exec |
| Control Plane    |
+------------------+
```

### Component Boundaries (New + Modified)

| Component | Status | Responsibility | Location |
|-----------|--------|---------------|----------|
| **Channel Router** | NEW | Maps named channels to `wait_for_callback()` + external delivery | `mcp_agent/channels/` |
| **Channel Config** | MODIFIED | Extends AgentConfig with channel definitions | `mcp_agent/config/types.rs` |
| **Channel Adapters** | NEW | Slack, Discord, WebSocket, Lambda Callback adapter implementations | `mcp_agent/channels/adapters/` |
| **Agent Team Orchestrator** | NEW | Dynamic MCP server generation, parallel agent invocation | `mcp_agent/teams/` |
| **Agent-as-MCP-Server** | NEW | Wraps a Durable Agent Lambda as an MCP tool | `mcp_agent/teams/agent_server.rs` |
| **Generic Agent Lambda** | MODIFIED | Config-driven agent: reads channel config, team config from registry | `mcp_agent/handler.rs` |
| **AgentRegistry Schema** | MODIFIED | New fields: `channels`, `team_config`, `agent_type` | DynamoDB (additive) |
| **pmcp-run Agents Tab** | NEW | UI pages for agent CRUD, execution, scheduling, history | `pmcp-run/app/(authenticated)/agents/` |
| **pmcp-run Agent API** | NEW | AppSync queries/mutations for agent management | `pmcp-run/amplify/data/resource.ts` |
| **PMCP SDK Example** | NEW | Reference example showing MCP Tasks + client patterns | `rust-mcp-sdk/examples/` |

---

## Component 1: Channels Layer

### Problem

The existing agent loop runs to completion without external interaction. `wait_for_callback()` exists in the SDK but requires the agent to know the AWS `SendDurableExecutionCallbackSuccess` API details. We need a named, config-driven abstraction that maps "ask the user on Slack for approval" to the right SDK primitive.

### Architecture

The Channel Router lives **inside the agent handler**, not as a separate service. It is a library module that the agent loop calls when it needs external interaction. The router:

1. Accepts a channel name (e.g., `"slack-approvals"`, `"discord-team"`)
2. Looks up the channel configuration from `AgentConfig.channels`
3. Sends the outbound message via the appropriate adapter
4. Calls `ctx.wait_for_callback()` to suspend the Lambda
5. Returns the external response when the callback completes

```
Agent Loop (handler.rs)
    |
    |-- needs approval for tool X
    |
    v
Channel Router (channels/router.rs)
    |
    |-- resolve channel name -> ChannelConfig
    |-- instantiate adapter (Slack, Discord, etc.)
    |-- send outbound message via adapter
    |-- call ctx.wait_for_callback() with submitter that delivers callback_id
    |-- Lambda suspends (zero cost)
    |
    ... external system receives message + callback_id ...
    ... external system calls SendDurableExecutionCallbackSuccess ...
    |
    v
Agent Loop resumes with callback result
```

### Channel Trait (Adapted from ZeroClaw)

ZeroClaw's `Channel` trait has `send()` and `listen()`. For Durable Lambda, `listen()` is not applicable -- the Lambda suspends rather than polling. We adapt the trait:

```rust
/// Durable-execution-compatible channel adapter.
///
/// Unlike zeroclaw's Channel trait which has `listen()` for long-running
/// polling, this trait only has `deliver()` -- the Lambda suspends via
/// `wait_for_callback()` rather than actively listening.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Human-readable channel type name.
    fn channel_type(&self) -> &str;

    /// Deliver a message to the external system with the callback_id
    /// so the external system knows how to respond.
    ///
    /// The adapter is responsible for formatting the message appropriately
    /// for its platform and including instructions for callback completion.
    async fn deliver(
        &self,
        message: &ChannelMessage,
        callback_id: &str,
    ) -> Result<(), ChannelError>;

    /// Optional health check.
    async fn health_check(&self) -> bool { true }
}
```

### Channel Adapters

| Adapter | Transport | How Callback Completes | Complexity |
|---------|-----------|----------------------|------------|
| **SlackAdapter** | Slack Web API (`chat.postMessage`) | Slack Bolt webhook -> API Gateway -> Lambda that calls `SendDurableExecutionCallbackSuccess` | Medium |
| **DiscordAdapter** | Discord REST API (`/channels/{id}/messages`) | Discord interaction webhook -> same callback Lambda | Medium |
| **WebhookAdapter** | Generic HTTP POST | Target system calls `SendDurableExecutionCallbackSuccess` directly or via a relay | Low |
| **LambdaCallbackAdapter** | Direct `SendDurableExecutionCallbackSuccess` | For programmatic callbacks (agent-to-agent, tests) | Low |

### Callback Relay Pattern

External systems (Slack, Discord) cannot call `SendDurableExecutionCallbackSuccess` directly -- they need an HTTP endpoint. The solution is a thin **Callback Relay Lambda** behind API Gateway:

```
Slack Button Click
    -> Slack sends interaction payload to API Gateway
    -> Callback Relay Lambda extracts callback_id from payload
    -> Calls lambda:SendDurableExecutionCallbackSuccess(callback_id, result)
    -> Agent Lambda resumes
```

The Callback Relay is a generic, reusable Lambda -- not per-agent. It validates the callback_id format and forwards. The `SendDurableExecutionCallbackSuccess` API endpoint is:

```
POST /2025-12-01/durable-execution-callbacks/{CallbackId}/succeed
Body: { "Result": <binary, max 256KB> }
```

### Channel Configuration (AgentRegistry Extension)

New `channels` field in AgentRegistry (JSON string, additive):

```json
{
  "channels": {
    "slack-approvals": {
      "type": "slack",
      "bot_token_secret": "/ai-agent/slack/bot-token",
      "default_channel_id": "C0123ABCDEF",
      "allowed_users": ["U12345", "U67890"],
      "message_template": "Agent {{agent_name}} needs approval:\n{{message}}\n\nCallback: {{callback_url}}"
    },
    "discord-alerts": {
      "type": "discord",
      "bot_token_secret": "/ai-agent/discord/bot-token",
      "channel_id": "1234567890",
      "webhook_url": "https://discord.com/api/webhooks/..."
    },
    "programmatic": {
      "type": "lambda_callback",
      "description": "Direct callback for automated workflows"
    }
  }
}
```

### Integration with Agent Loop

The channel router is called as a **tool** that the LLM can invoke, not hardcoded into the loop. This is the cleanest integration:

1. Add built-in channel tools alongside MCP-discovered tools during tool discovery
2. The LLM calls `channels__request_approval` (or similar) like any other tool
3. The tool execution handler recognizes channel tools, routes to the Channel Router
4. The Channel Router calls `ctx.wait_for_callback()`
5. The rest of the agent loop is unchanged

```rust
// In tool discovery phase, inject channel tools
fn inject_channel_tools(
    tools: &mut Vec<UnifiedTool>,
    routing: &mut HashMap<String, String>,
    channels: &HashMap<String, ChannelConfig>,
) {
    for (name, config) in channels {
        tools.push(UnifiedTool {
            name: format!("channels__send_{name}"),
            description: format!("Send a message to the {} channel and wait for a response", name),
            input_schema: channel_tool_schema(),
        });
        routing.insert(
            format!("channels__send_{name}"),
            format!("channel://{name}"),  // Special routing prefix
        );
    }
}
```

When tool execution encounters a `channel://` routing prefix, it delegates to the Channel Router instead of an MCP server. This keeps the agent loop handler untouched.

### Security Model

Following zeroclaw's deny-by-default pattern:

- Channels must be explicitly configured in AgentRegistry -- no implicit channel access
- Each channel config specifies `allowed_users` (who can respond)
- Callback Relay Lambda validates callback_id format before forwarding
- Channel secrets (bot tokens) stored in Secrets Manager, not in DynamoDB
- Timeout on `wait_for_callback()` prevents indefinite suspension (default 24h, configurable)

---

## Component 2: Agent Teams

### Problem

An orchestrator agent needs to delegate subtasks to specialist agents. In Step Functions, this requires complex state machine nesting. In Durable Lambda, `ctx.invoke()` can call another Lambda, but the calling agent needs to discover and describe the target agent's capabilities.

### Architecture: Agents as MCP Tools

The key insight: **an agent's capabilities can be described as an MCP tool**. An orchestrator agent discovers team members the same way it discovers MCP server tools -- through tool discovery. The difference is that "calling the tool" invokes another Durable Lambda agent instead of an MCP server.

```
Orchestrator Agent Lambda
    |
    |-- tool discovery phase:
    |   1. Discover MCP server tools (existing)
    |   2. Discover team member agents from config (NEW)
    |      Generate synthetic tool definitions for each team member
    |
    |-- agent loop:
    |   LLM sees tools: [mcp_server__tool_a, team__researcher, team__writer]
    |   LLM calls team__researcher with { task: "Research topic X" }
    |
    |-- tool execution:
    |   Routing detects "team://" prefix
    |   ctx.invoke() calls Researcher Agent Lambda
    |   Researcher runs its own agent loop (independent durable execution)
    |   Result returned to orchestrator
    |
    |-- orchestrator continues with result
```

### Team Configuration (AgentRegistry Extension)

New `team_config` field in AgentRegistry:

```json
{
  "team_config": {
    "role": "orchestrator",
    "members": [
      {
        "agent_name": "research-agent",
        "version": "v1",
        "tool_name": "researcher",
        "description": "Researches topics using web search and document analysis",
        "function_arn": "arn:aws:lambda:us-east-1:123456789:function:McpAgent"
      },
      {
        "agent_name": "writer-agent",
        "version": "v1",
        "tool_name": "writer",
        "description": "Writes structured documents from research findings",
        "function_arn": "arn:aws:lambda:us-east-1:123456789:function:McpAgent"
      }
    ],
    "shared_context": {
      "strategy": "summary",
      "max_context_tokens": 4000
    }
  }
}
```

### Team Member Tool Generation

During tool discovery, the orchestrator generates synthetic tool definitions for each team member:

```rust
fn generate_team_tools(team_config: &TeamConfig) -> Vec<(UnifiedTool, String)> {
    team_config.members.iter().map(|member| {
        let tool = UnifiedTool {
            name: format!("team__{}", member.tool_name),
            description: member.description.clone(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task to delegate to this agent"
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional context for the agent"
                    }
                },
                "required": ["task"]
            }),
        };
        let routing = format!("team://{}/{}", member.agent_name, member.version);
        (tool, routing)
    }).collect()
}
```

### Team Member Invocation

When the LLM calls a `team://` tool, the handler uses `ctx.invoke()` to call the member agent's Lambda:

```rust
async fn execute_team_call(
    ctx: &DurableContextHandle,
    member: &TeamMember,
    task: &str,
    shared_context: Option<&str>,
) -> DurableResult<String> {
    let request = AgentRequest {
        agent_name: member.agent_name.clone(),
        version: member.version.clone(),
        messages: vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: format_team_task(task, shared_context),
            },
        }],
    };

    // ctx.invoke() calls the member Lambda and waits for completion
    // The member runs its own independent durable execution
    let response: AgentResponse = ctx
        .invoke(
            Some(&format!("team-{}", member.tool_name)),
            &member.function_arn,
            &serde_json::to_value(&request)?,
            None,
        )
        .await?;

    // Extract text response from the member agent
    extract_text_response(&response)
}
```

### Shared Context Strategy

Orchestrator-to-member context sharing has three options:

| Strategy | How | When |
|----------|-----|------|
| **Full pass-through** | Send orchestrator's full message history to member | Small conversations, high coherence needed |
| **Summary** | LLM-generated summary injected as context | Large conversations, token budget concerns |
| **Task-only** | Only the task description, no conversation history | Independent subtasks |

Default to **task-only** for v2.0. Summary and full pass-through add complexity and checkpoint size concerns. The task description is usually sufficient for delegation.

### Single Lambda, Multiple Agents

A critical design point: all agents (orchestrator and members) can use the **same Lambda function**. The `agent_name` in the request determines which AgentRegistry config to load. There is no need for separate Lambda functions per agent -- the config-driven approach means one Lambda binary serves all agents. The `function_arn` in team config points to the same Lambda.

---

## Component 3: pmcp-run Agents Tab

### Problem

Agents need a management UI. The Step Functions Agent project has a rich Amplify Gen 2 UI (12+ pages). Rather than rebuilding, we extend pmcp-run (which already has registry, deployment, and monitoring) with an Agents tab.

### Data Model Additions (pmcp-run AppSync Schema)

New models added to `pmcp-run/amplify/data/resource.ts`:

```typescript
// In the existing schema, add:

AgentConfig: a.model({
    id: a.id().required(),
    organizationId: a.string().required(),

    // Core
    agentName: a.string().required(),
    version: a.string().required(),
    displayName: a.string(),
    description: a.string(),
    status: a.enum(['active', 'inactive', 'draft']),
    agentType: a.enum(['standalone', 'orchestrator', 'team_member']),

    // LLM Configuration
    llmProvider: a.string().required(),     // "anthropic" | "openai"
    llmModel: a.string().required(),        // "claude-sonnet-4-20250514"
    systemPrompt: a.string().required(),
    parameters: a.json(),                    // AgentParameters JSON

    // MCP Servers (references to McpServer records)
    mcpServerIds: a.string().array(),        // Links to McpServer.id
    mcpServerUrls: a.json(),                 // Resolved URLs (denormalized for Lambda)

    // Channels
    channels: a.json(),                      // Channel configurations

    // Team Configuration
    teamConfig: a.json(),                    // TeamConfig JSON

    // Deployment
    lambdaFunctionArn: a.string(),           // Deployed Lambda ARN
    lambdaFunctionName: a.string(),
    deploymentStatus: a.enum(['not_deployed', 'deploying', 'deployed', 'failed']),
    lastDeployedAt: a.datetime(),

    // Scheduling
    scheduleExpression: a.string(),          // EventBridge cron/rate expression
    scheduleEnabled: a.boolean().default(false),
    scheduleInput: a.json(),                 // Default AgentRequest for scheduled runs

    // Metadata
    createdAt: a.datetime(),
    updatedAt: a.datetime(),
    createdBy: a.string(),

    // Relationships
    organization: a.belongsTo('Organization', 'organizationId'),
    executions: a.hasMany('AgentExecution', 'agentConfigId'),
})
.authorization((allow) => [allow.authenticated()]),

AgentExecution: a.model({
    id: a.id().required(),
    organizationId: a.string().required(),
    agentConfigId: a.string().required(),

    // Execution
    durableExecutionArn: a.string(),         // AWS Durable Execution ARN
    status: a.enum(['running', 'succeeded', 'failed', 'suspended', 'timed_out']),
    startedAt: a.datetime(),
    completedAt: a.datetime(),

    // Input/Output
    inputMessages: a.json(),                 // Initial messages
    outputResponse: a.json(),                // AgentResponse JSON
    errorMessage: a.string(),

    // Metadata
    iterations: a.integer(),
    totalInputTokens: a.integer(),
    totalOutputTokens: a.integer(),
    toolsCalled: a.string().array(),
    elapsedMs: a.integer(),

    // Relationships
    agentConfig: a.belongsTo('AgentConfig', 'agentConfigId'),
    organization: a.belongsTo('Organization', 'organizationId'),
})
.authorization((allow) => [allow.authenticated()]),
```

### Relationship to Existing AgentRegistry

The pmcp-run `AgentConfig` model is the **source of truth** for agent configuration. The DynamoDB AgentRegistry table (used by the Lambda) is a **deployment artifact** -- when an agent is deployed via pmcp-run, its config is written to the AgentRegistry table in the format the Lambda expects.

```
pmcp-run AgentConfig (AppSync/DynamoDB)
    |
    |-- user creates/edits agent via UI
    |-- includes mcpServerIds (links to pmcp-run McpServer records)
    |
    v
Deploy Agent (Lambda function)
    |
    |-- resolve mcpServerIds -> MCP server URLs
    |-- transform AgentConfig -> AgentRegistry item format
    |-- write to AgentRegistry DynamoDB table
    |-- update Lambda environment if needed
    |
    v
AgentRegistry DynamoDB Table (agent_name PK, version SK)
    |
    |-- Lambda reads at runtime via load_agent_config()
```

This two-table approach avoids modifying the Lambda's config loading code. The pmcp-run model is richer (organization scoping, deployment status, scheduling) while the AgentRegistry table is flat and fast for Lambda reads.

### UI Pages

| Page | Route | Purpose |
|------|-------|---------|
| Agent List | `/agents` | List all agents with status, last execution, quick actions |
| Agent Detail | `/agents/[id]` | View config, recent executions, metrics |
| Agent Editor | `/agents/[id]/edit` | Edit system prompt, model, MCP servers, channels, team config |
| Agent Create | `/agents/new` | Wizard: choose type (standalone/orchestrator/member), configure |
| Execution Detail | `/agents/[id]/executions/[execId]` | Durable execution trace, messages, tool calls, channel interactions |
| Execution History | `/agents/[id]/executions` | List past executions with filters |

### MCP Server Linking

The pmcp-run Agents tab reuses the existing `McpServer` model. When configuring an agent's MCP servers, the UI presents a picker showing deployed MCP servers from the user's organization. Selected servers are stored as `mcpServerIds`. At deploy time, the server endpoints are resolved and written to `mcp_servers` in the AgentRegistry.

This creates a natural bridge: pmcp-run hosts MCP servers AND manages the agents that use them.

---

## Component 4: PMCP SDK Reference Example

### Problem

The PMCP SDK (`rust-mcp-sdk` / `pmcp` crate) needs a reference example showing MCP Tasks and client patterns. This is separate from the agent binary but demonstrates how durable agents interact with task-capable MCP servers.

### Architecture

The example consists of:

1. **Task-capable MCP Server** -- an MCP server built with `pmcp` that returns `Task` objects for long-running operations
2. **Durable Client Example** -- a Durable Lambda that uses `pmcp` Client's `call_tool_with_task()` and polls `tasks_get()` via `ctx.step()` loops

The `pmcp` Client already has the Task API surface (verified from source):
- `call_tool_with_task()` returns `ToolCallResponse::Task(Task)` or `ToolCallResponse::Result(CallToolResult)`
- `tasks_get(task_id)` returns `GetTaskResult` with `TaskStatus`
- `tasks_result(task_id)` returns the final payload

```rust
// Durable handler that works with MCP Tasks
let response = ctx.step(Some("start-long-task"), |_| async {
    let client = create_mcp_client(&server_url).await?;
    match client.call_tool_with_task("analyze_document", input).await? {
        ToolCallResponse::Result(result) => Ok(TaskOrResult::Result(result)),
        ToolCallResponse::Task(task) => Ok(TaskOrResult::Task(task.id)),
    }
}, None).await?;

match response {
    TaskOrResult::Task(task_id) => {
        // Poll until complete using wait_for_condition
        let result = ctx.wait_for_condition(
            Some("poll-task"),
            move |_| {
                let id = task_id.clone();
                let url = server_url.clone();
                async move {
                    let client = create_mcp_client(&url).await?;
                    let status = client.tasks_get(&id).await?;
                    match status.task.status {
                        TaskStatus::Completed => Ok(WaitConditionDecision::Complete(status)),
                        TaskStatus::Failed => Err("Task failed"),
                        _ => Ok(WaitConditionDecision::Continue),
                    }
                }
            },
            Some(WaitConditionConfig::new()
                .with_interval(Duration::seconds(5))
                .with_timeout(Duration::minutes(30))),
        ).await?;
        // Use result
    }
    TaskOrResult::Result(result) => { /* immediate result */ }
}
```

---

## Cross-System Data Flow

### How Systems Relate

```
Step Functions Agent (EXISTING)     pmcp-run (EXISTING)        Durable Agent (THIS REPO)
==========================          ================           =======================
AgentRegistry DynamoDB     <---deploy-from---- AgentConfig model     --reads--> AgentRegistry
ToolRegistry DynamoDB      (deprecated)        (use MCP instead)
MCPServerRegistry DynamoDB <---migrate-to----> McpServer model       --connects-> MCP servers
LLMModels DynamoDB         <---migrate-to----> (in AgentConfig)
TemplateRegistry DynamoDB  (not needed)

Step Functions State Machine   (replaced by)   Durable Lambda
CDK Python deployment          (replaced by)   SAM + pmcp-run deploy
Amplify Gen 2 UI               (migrated to)   pmcp-run Agents tab
```

### Migration Path from Step Functions Agent

The Step Functions Agent has these DynamoDB tables:
- **AgentRegistry**: `agent_name` (PK), `version` (SK) -- system_prompt, llm_provider, llm_model, tools, state_machine_arn, observability, metadata
- **ToolRegistry**: Individual tool definitions with Lambda ARNs
- **MCPServerRegistry**: `server_id` (PK), `version` (SK) -- endpoint_url, available_tools, authentication_type
- **LLMModels**: Provider-keyed model catalog

For the Durable Agent:
- **AgentRegistry** is reused with additive fields (`channels`, `team_config`, `mcp_servers`)
- **ToolRegistry** is eliminated -- tools come from MCP servers via `list_tools()`
- **MCPServerRegistry** maps to pmcp-run's `McpServer` model -- the deployed server URL IS the MCP endpoint
- **LLMModels** is simplified to `llm_provider` + `llm_model` strings in AgentConfig

### Channel Data Flow

```
1. User configures channel in pmcp-run UI
   -> writes to AgentConfig.channels (AppSync)

2. Agent deploys
   -> channels config written to AgentRegistry DynamoDB
   -> channel secrets referenced (not copied) from Secrets Manager

3. Agent runs, LLM decides to use a channel tool
   -> Channel Router reads channel config from loaded AgentConfig
   -> Adapter sends message to external system (Slack API, etc.)
   -> ctx.wait_for_callback() suspends Lambda

4. External user responds
   -> Slack interaction webhook -> API Gateway -> Callback Relay Lambda
   -> Callback Relay calls SendDurableExecutionCallbackSuccess(callback_id, result)
   -> Durable Execution control plane resumes agent Lambda

5. Agent resumes with callback result
   -> Channel Router returns result to agent loop
   -> LLM continues reasoning with the response
```

### Team Invocation Data Flow

```
1. Orchestrator agent receives task
   -> loads config including team_config
   -> generates team member tool definitions

2. LLM calls team__researcher tool
   -> handler detects team:// routing prefix
   -> builds AgentRequest for member agent

3. ctx.invoke() calls the member Lambda
   -> member Lambda is the SAME Lambda binary
   -> member loads its own AgentConfig (different agent_name)
   -> member runs its own independent durable execution
   -> member returns AgentResponse

4. Orchestrator receives member's result
   -> result checkpointed as ctx.invoke() output
   -> LLM continues with the research findings
```

---

## Patterns to Follow

### Pattern 1: Channel as Tool (not Loop Injection)

**What:** Expose channels as tools the LLM can call, rather than hardcoding channel interactions into the agent loop.

**When:** Always. This is the recommended integration pattern.

**Why:** The LLM decides when to ask for approval or send messages. The agent loop code stays clean -- it does not need channel-specific branching. New channel types are added by configuration, not code changes.

```rust
// During tool discovery, inject channel tools
let mut tools_with_routing = discover_mcp_tools(&config.mcp_server_urls).await?;

if let Some(ref channels) = config.channels {
    inject_channel_tools(&mut tools_with_routing, channels);
}
```

### Pattern 2: Callback Relay for External Systems

**What:** A thin, generic Lambda behind API Gateway that translates external webhook payloads into `SendDurableExecutionCallbackSuccess` calls.

**When:** Any channel adapter that connects to an external system (Slack, Discord, custom webhooks).

**Why:** External systems cannot call the Lambda API directly. The relay is stateless and reusable across all agents.

```rust
// Callback Relay Lambda handler (separate from agent Lambda)
async fn callback_relay_handler(event: ApiGatewayEvent) -> Result<Response> {
    let callback_id = extract_callback_id(&event)?;
    let result = extract_result_payload(&event)?;

    let client = aws_sdk_lambda::Client::new(&aws_config);
    client.send_durable_execution_callback_success()
        .callback_id(&callback_id)
        .result(Blob::new(result))
        .send()
        .await?;

    Ok(Response::ok())
}
```

### Pattern 3: Config-Driven Agent Binary

**What:** A single Lambda binary that reads all behavior from AgentRegistry. Agent type (standalone, orchestrator, team member), channels, MCP servers, and team config are all determined at runtime from config.

**When:** Always. Do not create separate Lambda binaries for different agent types.

**Why:** Deployment simplicity. One `sam deploy` for all agents. New agents are created by adding AgentRegistry items, not deploying new code.

### Pattern 4: Team Member via ctx.invoke()

**What:** Orchestrator agents call team members via `ctx.invoke()`, which creates a new durable execution for the member.

**When:** Agent Teams feature.

**Why:** Each member runs in its own durable execution with independent checkpointing. If the member fails, it can be retried without re-running the orchestrator. The member's execution history is separate and inspectable.

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Channels as Middleware

**What:** Inserting a channel middleware layer that intercepts every tool call or every LLM response.

**Why bad:** Over-engineering. Not every tool call needs approval. The LLM should decide when to use channels, not a middleware layer. This also breaks the deterministic handler requirement -- middleware state between steps is not checkpointed.

**Instead:** Channels are tools. The LLM calls them explicitly when it decides external input is needed.

### Anti-Pattern 2: Persistent Channel Connections

**What:** Maintaining WebSocket or SSE connections to Slack/Discord within the agent Lambda.

**Why bad:** Lambda suspends during `wait_for_callback()`. Persistent connections die on suspension. On replay, connection state is lost. This is the same problem as MCP connections (Pitfall 3 from v1.0 research).

**Instead:** Adapters use stateless HTTP calls (Slack Web API POST, Discord REST API POST). The callback return path uses API Gateway + Callback Relay.

### Anti-Pattern 3: Shared State Between Orchestrator and Members

**What:** Using DynamoDB or S3 as a shared scratchpad between orchestrator and team member agents during execution.

**Why bad:** Breaks durable execution's replay safety. If the shared state is modified during replay, members see stale or inconsistent data. Adds hidden dependencies between independent executions.

**Instead:** Pass context as part of the `AgentRequest.messages` when invoking members. All state flows through the durable execution's checkpoint mechanism.

### Anti-Pattern 4: Direct DynamoDB Access from pmcp-run UI to AgentRegistry

**What:** Having the pmcp-run UI read/write directly to the AgentRegistry DynamoDB table that the Lambda uses.

**Why bad:** The AgentRegistry table has a simple flat schema optimized for Lambda's GetItem. The pmcp-run UI needs rich queries (list by organization, filter by status, pagination). Two different access patterns on the same table creates contention and index bloat.

**Instead:** Two-table design. pmcp-run has its own `AgentConfig` model (AppSync/DynamoDB) for CRUD. Deploy action writes a flattened copy to AgentRegistry for the Lambda.

---

## Scalability Considerations

| Concern | 1 Agent | 10 Agents | 100+ Agents |
|---------|---------|-----------|-------------|
| Lambda concurrency | 1 concurrent execution per active agent | 10 concurrent | Request limit increase; each agent is independent |
| AgentRegistry reads | 1 GetItem per execution (cached by step) | 10 total | Use DAX if needed; reads are by PK, very fast |
| Channel interactions | Rare (few approvals per run) | ~10 pending callbacks at peak | Callback Relay Lambda scales automatically |
| Team invocations | N/A | 2-5 member calls per orchestrator run | ctx.invoke() creates independent executions; scales linearly |
| Checkpoint storage | ~225KB per 10-iteration run | Independent per execution | No cross-execution contention |
| pmcp-run API load | Minimal (config reads) | ~100 API calls/hour for monitoring | AppSync scales; DynamoDB on-demand |
| Callback Relay | Single Lambda, rarely invoked | Handles all agents' callbacks | API Gateway + Lambda scales to thousands/sec |

---

## Suggested Build Order

Based on component dependencies, the v2.0 features should be built in this order:

```
Phase 6: Generic Agent Binary
  - Make agent binary fully config-driven (remove hardcoded assumptions)
  - Add agent_type field to AgentConfig
  - Refactor handler to support extension points (channel tools, team tools)
  Dependencies: v1.0 complete (confirmed)
  Risk: Low -- refactoring existing working code

Phase 7: Channels Foundation
  - ChannelAdapter trait
  - Channel Router (resolve name -> adapter)
  - Channel tool injection during tool discovery
  - LambdaCallbackAdapter (programmatic, for testing)
  - CallbackConfig integration with wait_for_callback()
  - Unit tests with mock callback
  Dependencies: Phase 6 (extension points in handler)
  Risk: Medium -- wait_for_callback() integration needs validation

Phase 8: Channel Adapters + Callback Relay
  - SlackAdapter (chat.postMessage + interaction payload format)
  - Callback Relay Lambda + API Gateway (SAM resources)
  - DiscordAdapter
  - WebhookAdapter
  - End-to-end test: agent sends Slack message, user responds, agent resumes
  Dependencies: Phase 7 (ChannelAdapter trait)
  Risk: Medium -- external API integration, webhook setup

Phase 9: Agent Teams
  - TeamConfig parsing from AgentRegistry
  - Team member tool generation
  - ctx.invoke() integration for member calls
  - Shared context formatting
  - Test: orchestrator delegates to two members
  Dependencies: Phase 6 (generic agent binary -- members are the same Lambda)
  Risk: Medium -- ctx.invoke() behavior with durable execution needs validation

Phase 10: pmcp-run Agents Tab
  - AgentConfig + AgentExecution models in AppSync schema
  - Agent CRUD pages (list, create, edit, detail)
  - MCP server picker (reuse existing McpServer records)
  - Deploy action (write to AgentRegistry)
  - Execution history page
  Dependencies: Phases 6-9 (agent features to manage), pmcp-run codebase
  Risk: Low -- standard Amplify CRUD, follows existing patterns

Phase 11: PMCP SDK Example
  - Task-capable MCP server example
  - Durable client example using call_tool_with_task + wait_for_condition
  - Documentation
  Dependencies: pmcp crate Task API (verified exists in source)
  Risk: Low -- example code, not production infrastructure
```

**Critical path:** Phase 6 -> Phase 7 -> Phase 8 (channels) and Phase 6 -> Phase 9 (teams) can run in parallel after Phase 6. Phase 10 can start after Phase 6 for basic UI, but needs Phase 7-9 for channel and team config editing. Phase 11 is independent.

**Parallelizable:** Phases 7 and 9 after Phase 6 is complete. Phase 11 is fully independent.

---

## Sources

### Primary (HIGH confidence)
- Durable Execution SDK source: `src/context/durable_context/callback.rs`, `callback/execute.rs` -- wait_for_callback() API, CallbackHandle, submitter pattern (read directly)
- Agent binary source: `examples/src/bin/mcp_agent/` -- handler.rs, config/, mcp/, llm/, types.rs (read directly)
- SAM template: `examples/template-agent.yaml` -- deployment pattern (read directly)
- ZeroClaw channels: `~/projects/LocalAgent/zeroclaw/src/channels/traits.rs`, `activity.rs`, `slack.rs` -- Channel trait, adapter patterns (read directly)
- pmcp-run schema: `~/Development/mcp/sdk/pmcp-run/amplify/data/resource.ts` -- existing data model, McpServer, Organization, Deployment (read directly)
- Step Functions Agent registry: `~/projects/step-functions-agent/ui_amplify/amplify/data/resource.ts` -- Agent, MCPServer, LLMModel custom types (read directly)
- PMCP SDK client: `~/Development/mcp/sdk/rust-mcp-sdk/src/client/mod.rs` -- Client struct, ToolCallResponse enum, Task API surface (read directly)
- [SendDurableExecutionCallbackSuccess API](https://docs.aws.amazon.com/lambda/latest/api/API_SendDurableExecutionCallbackSuccess.html) -- POST /2025-12-01/durable-execution-callbacks/{CallbackId}/succeed, 256KB result limit

### Secondary (MEDIUM confidence)
- [AWS Lambda Durable Functions docs](https://docs.aws.amazon.com/lambda/latest/dg/durable-functions.html) -- callback lifecycle, execution model
- [AWS blog: Build multi-step applications with durable functions](https://aws.amazon.com/blogs/aws/build-multi-step-applications-and-ai-workflows-with-aws-lambda-durable-functions/) -- architecture patterns
- [MCP specification 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25) -- Tasks primitive definition
- pmcp-run architecture: `~/Development/mcp/sdk/pmcp-run/ARCHITECTURE.md` -- multi-tenant strategy, deployment pipeline

### Tertiary (LOW confidence)
- [MCP Tasks blog](https://workos.com/blog/mcp-async-tasks-ai-agent-workflows) -- Tasks lifecycle and polling patterns (experimental feature)
- [2026 MCP Roadmap](http://blog.modelcontextprotocol.io/posts/2026-mcp-roadmap/) -- Tasks stability and future direction
- ctx.invoke() behavior with same-Lambda invocation -- not tested in existing examples; needs validation
- Callback Relay Lambda interaction with Slack/Discord webhook formats -- inferred from zeroclaw patterns, not validated
