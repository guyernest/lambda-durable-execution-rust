# Technology Stack

**Project:** Durable Lambda MCP Agent Platform v2.0
**Researched:** 2026-03-24
**Overall Confidence:** HIGH (versions verified against crates.io, patterns validated against zeroclaw reference implementation and pmcp-run codebase)

## Scope

This document covers stack additions for v2.0 milestone features only:
1. Channels abstraction (Slack, Discord, WhatsApp, Signal, local agent, webhooks)
2. Agent Teams (dynamic MCP server generation, orchestrator pattern)
3. pmcp-run integration (Agents tab in Amplify Gen 2 + Next.js platform)
4. PMCP SDK example (reference example packaging)
5. EventBridge/scheduled triggers

Already-validated v1.0 stack (NOT re-researched): `lambda-durable-execution-rust`, `lambda_runtime`, `tokio`, `serde`/`serde_json`, `aws-config`, `aws-sdk-lambda`, `aws-sdk-dynamodb`, `aws-sdk-secretsmanager`, `reqwest`, `pmcp`, `thiserror`, `tracing`, `chrono`, `uuid`.

---

## Recommended Stack Additions

### Channels System (Rust Agent Binary)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `reqwest` | 0.13.2 (already resolved) | HTTP client for Slack, WhatsApp, Signal APIs | Already in lockfile. All platform APIs are REST/HTTP -- no dedicated SDK crate needed. Zeroclaw validates this pattern across 12+ channels |
| NO new channel crates | -- | -- | DO NOT add `slack-morphism`, `serenity`, or platform-specific SDK crates. The channel trait is thin (send/health_check) and Lambda does not need long-running listeners. Raw `reqwest` is the right choice because: (1) Lambda channels only need `send()`, not `listen()`, since messages arrive via `wait_for_callback`; (2) platform SDKs add heavy dependency trees for features we don't use; (3) zeroclaw proves reqwest-only works for Slack, WhatsApp, and Telegram |

**Key Architectural Decision -- Lambda Channels Are Not Zeroclaw Channels:**

Zeroclaw's `Channel` trait has `listen()` (long-running polling/WebSocket loop) and `send()`. Lambda channels only need `send()` because incoming messages arrive via `SendDurableExecutionCallbackSuccess` -- the Lambda is suspended, not polling. The `wait_for_callback` SDK primitive IS the "listen" mechanism.

Therefore:
- **No `tokio-tungstenite`** -- Discord gateway WebSocket is not viable in Lambda (Lambda suspends between callbacks; you cannot hold a WebSocket open). Discord messages come in via webhook -> API Gateway -> `SendDurableExecutionCallbackSuccess`.
- **No `serenity`** -- Same reason. Gateway bot pattern requires persistent connection.
- **No `slack-morphism`** -- Slack Socket Mode requires persistent WebSocket. Slack Events API webhook -> callback is the Lambda-compatible pattern.

The channel `send()` implementations use `reqwest` to call platform REST APIs (Slack Web API, Discord REST API, WhatsApp Cloud API). The "receive" side is handled by external webhook receivers that call `SendDurableExecutionCallbackSuccess`.

### Webhook Receiver (Separate Lambda or API Gateway Integration)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `aws-sdk-lambda` | 1.112.0 (already in use) | Call `SendDurableExecutionCallbackSuccess` from webhook handler | Already a dependency; webhook receiver invokes callback API |
| `lambda_http` | 1.1.2 | API Gateway Lambda handler for webhooks | Standard crate for Lambda functions receiving HTTP events from API Gateway. Webhook receiver is a SEPARATE Lambda (not the durable agent). Same version family as `lambda_runtime` 1.1.2 |
| `hmac` | 0.12.1 | Webhook signature verification (HMAC-SHA256) | Slack requires HMAC-SHA256 signature verification on incoming webhooks. `sha2` 0.10 is already in the SDK's dependencies |

The webhook receiver pattern:
1. Platform (Slack/Discord/WhatsApp) sends event to API Gateway
2. API Gateway invokes a lightweight webhook receiver Lambda
3. Webhook Lambda validates the event (HMAC-SHA256 for Slack, Ed25519 for Discord, HMAC-SHA256 for WhatsApp), extracts message, calls `SendDurableExecutionCallbackSuccess` with the callback ID
4. Durable agent Lambda resumes from `wait_for_callback`

### Agent Teams (Dynamic MCP Server)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `pmcp` | 2.0.2 (path dep, already in use) | `DynamicServerManager` for runtime tool registration | pmcp's `server::dynamic::DynamicServerManager` supports adding/removing tools at runtime. Agent teams generate MCP servers where each team member agent is exposed as a tool. Already verified in pmcp source |
| `pmcp` features: `streamable-http` | (already enabled) | HTTP/SSE transport for dynamic MCP server | Agent-as-MCP-server needs HTTP transport for other agents to connect. `streamable-http` feature already enabled in examples/Cargo.toml |

**No new dependencies needed for Agent Teams.** The `pmcp` crate's `DynamicServerManager` + `ServerCoreBuilder` provide everything:
- `DynamicServerManager::add_tool()` registers agent-as-tool handlers at runtime
- `DynamicConfigBuilder` creates tool configurations with schemas
- The orchestrator agent connects to team member agents via the existing `pmcp` client

The orchestrator pattern:
1. Load team configuration from AgentRegistry (team members, roles)
2. For each team member, create a `ToolHandler` that invokes the member's agent Lambda via `ctx.invoke()`
3. Register all tools on a `DynamicServerManager`
4. Orchestrator LLM sees team members as tools and delegates naturally

### EventBridge Scheduled Triggers

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `aws-sdk-scheduler` | ~1.97 | Create/manage EventBridge Scheduler schedules programmatically | EventBridge Scheduler (not EventBridge rules) is the right choice for one-time and recurring agent triggers. Supports flexible time windows, one-time schedules, and cron/rate expressions. Verified on crates.io: v1.97.0 (March 2026) |

**Use `aws-sdk-scheduler`, NOT `aws-sdk-eventbridge`.** EventBridge Scheduler is the dedicated scheduling service with richer features (one-time schedules, flexible time windows, built-in retry policies). EventBridge rules are for event routing, not scheduling. The Scheduler can directly invoke Lambda functions as targets.

SAM template additions (declarative, no Rust code needed for basic schedules):
```yaml
AgentSchedule:
  Type: AWS::Scheduler::Schedule
  Properties:
    ScheduleExpression: "rate(1 hour)"
    FlexibleTimeWindow:
      Mode: "OFF"
    Target:
      Arn: !GetAtt DurableAgentFunction.Arn
      Input: '{"agent_name": "scheduled-agent", "version": "v1", "messages": [...]}'
```

For dynamic schedule management (create/update/delete from the Agents tab), the `aws-sdk-scheduler` crate is needed in a pmcp-run Amplify function.

### pmcp-run Integration (Next.js + Amplify Gen 2)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Amplify Gen 2 data schema | (existing) | Add `Agent` and `AgentExecution` models to existing schema | pmcp-run already has Amplify Gen 2 with DynamoDB models. Agent config extends the existing pattern |
| Amplify Gen 2 functions | (existing) | Lambda functions for agent CRUD, schedule management | Same pattern as existing `uploadDeployment`, `triggerBuild` etc. |
| Next.js App Router | (existing) | Agents tab pages under `app/(authenticated)/agents/` | pmcp-run uses Next.js App Router with LCARS UI components |
| `@aws-sdk/client-scheduler` | ^3.x | Schedule management from Amplify functions | Create/update/delete EventBridge Scheduler schedules via Agents tab |

**No new npm packages needed for the frontend.** pmcp-run already has:
- `@aws-amplify/backend` for data schema definitions
- `@aws-sdk/client-lambda` (likely, for existing deployment functions)
- Next.js + React for UI
- LCARS UI component library (custom)

Frontend additions for the Agents tab:
1. Add `{ name: 'Agents', href: '/agents', color: 'green' as const }` to `authentic-navigation.tsx`
2. Create `app/(authenticated)/agents/page.tsx` (list view)
3. Create `app/(authenticated)/agents/[id]/page.tsx` (detail/edit view)
4. Create `app/(authenticated)/agents/[id]/executions/page.tsx` (execution history)

Schema additions to `amplify/data/resource.ts`:
```typescript
Agent: a.model({
  id: a.id().required(),
  organizationId: a.string().required(),
  name: a.string().required(),
  version: a.string().required(),
  systemPrompt: a.string(),
  providerId: a.string(),    // "anthropic" | "openai"
  modelId: a.string(),
  mcpServerIds: a.string().array(),  // References to McpServer.id
  channels: a.json(),         // Channel configs
  teamMembers: a.json(),      // Agent team member references
  parameters: a.json(),       // AgentParameters
  scheduleArn: a.string(),    // EventBridge Scheduler ARN
  scheduleExpression: a.string(),
  status: a.enum(['active', 'inactive', 'error']),
  lastExecutionAt: a.datetime(),
  // ...standard metadata
})
```

### PMCP SDK Example

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `pmcp` | 2.0.2 (path dep) | Build the durable agent as a reference MCP client example | The agent binary IS an MCP client example. Package it as a numbered pmcp example (e.g., `examples/70_durable_agent/`) |
| `lambda-durable-execution-rust` | 0.1.0 (path dep) | SDK dependency for the example | Already the pattern in this repo's examples/ |

No new dependencies. The PMCP SDK example is a packaging/documentation exercise, not a code exercise. The existing `mcp_agent` binary already demonstrates: MCP client connection, tool discovery, tool execution, streamable-http transport.

---

## Dependencies NOT to Add

| Crate | Why Not |
|-------|---------|
| `slack-morphism` (2.19.0) | Adds ~15 transitive deps for Socket Mode, Block Kit, events framework. Lambda only needs `reqwest::Client::post("https://slack.com/api/chat.postMessage")`. Overkill |
| `serenity` (0.12.5) | Discord bot framework requiring persistent gateway WebSocket. Lambda cannot maintain WebSocket connections across suspensions |
| `tokio-tungstenite` (0.29.0) | No WebSocket connections needed from the agent Lambda. All platform integrations use REST APIs for outbound, webhooks for inbound |
| `aws-sdk-eventbridge` (1.104.0) | EventBridge rules are for event routing. Use `aws-sdk-scheduler` for scheduled triggers -- purpose-built for scheduling with richer features |
| `aws-sdk-sfn` | Step Functions is being REPLACED by Durable Lambda. Do not add SFN dependencies |
| `anyhow` | Project uses `thiserror` for typed errors. Channel errors should be typed (`ChannelError::SlackApiError`, `ChannelError::WebhookVerificationFailed`), not opaque |
| `axum` (directly in agent binary) | The agent Lambda is invoked by Lambda runtime, not by HTTP requests. `axum` is already a transitive dep via pmcp's `streamable-http` feature but should not be a direct dependency of the agent |

---

## Full Dependency Manifest (v2.0 Agent Binary)

```toml
# examples/Cargo.toml additions for v2.0

[dependencies]
# Already present (v1.0) -- no changes needed:
lambda-durable-execution-rust = { path = ".." }
tokio = { version = "1", features = ["full", "sync", "time", "macros"] }
async-trait = "0.1"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
lambda_runtime = "1.0.1"
chrono = { version = "0.4", features = ["serde"] }
reqwest = { version = "0.13", features = ["json"] }
aws-config = { version = "1.8", features = ["behavior-version-latest"] }
aws-sdk-dynamodb = "1.98"
aws-sdk-secretsmanager = "1.98"
thiserror = "2.0"
pmcp = { path = "../../sdk/rust-mcp-sdk", features = ["streamable-http"] }
url = "2.5"

# NEW for v2.0:
lambda_http = "1"    # Webhook receiver Lambda (API Gateway events)
hmac = "0.12"        # Webhook signature verification (Slack HMAC-SHA256)
```

**Key insight:** The v2.0 agent binary itself needs zero new Cargo dependencies. Channel `send()` uses the existing `reqwest`. Channel `receive` uses the existing `wait_for_callback` SDK primitive. Agent Teams uses the existing `pmcp` `DynamicServerManager`. The only new deps are for the webhook receiver binary (`lambda_http`, `hmac`), which shares the same `examples/Cargo.toml`.

### Webhook Receiver Binary (NEW binary, same Cargo.toml)

```toml
# New binary in examples/Cargo.toml
[[bin]]
name = "channel_webhook_receiver"
path = "src/bin/channel_webhook_receiver/main.rs"
```

### pmcp-run Amplify Function Dependencies (TypeScript)

```json
{
  "@aws-sdk/client-scheduler": "^3.x",
  "@aws-sdk/client-lambda": "^3.x"
}
```

These are added to specific Amplify function `package.json` files, not the root project.

---

## Version Verification Matrix

| Crate | Claimed | Verified Source | Verified Version | Risk |
|-------|---------|-----------------|------------------|------|
| `reqwest` | 0.13.2 | crates.io API | 0.13.2 | NONE -- already in lockfile |
| `pmcp` | 2.0.2 | crates.io API | 2.0.2 | NONE -- path dep, verified |
| `aws-sdk-scheduler` | ~1.97 | crates.io API | 1.97.0 | LOW -- standard AWS SDK versioning |
| `lambda_http` | 1.1.2 | crates.io API | 1.1.2 (March 2026) | LOW -- same version family as lambda_runtime |
| `hmac` | 0.12.1 | crates.io API | 0.12.1 (stable) | LOW -- mature RustCrypto crate |
| `aws-sdk-eventbridge` | ~1.104 | crates.io API (NOT RECOMMENDED) | 1.104.0 | N/A -- not adding |
| `tokio-tungstenite` | 0.29.0 | crates.io API (NOT RECOMMENDED) | 0.29.0 | N/A -- not adding |
| `slack-morphism` | 2.19.0 | crates.io API (NOT RECOMMENDED) | 2.19.0 | N/A -- not adding |
| `aws-sdk-dynamodb` | 1.98 (existing) | crates.io API shows 1.110.0 | Upgrade optional | LOW -- patch versions |
| `aws-sdk-secretsmanager` | 1.98 (existing) | crates.io API shows 1.103.0 | Upgrade optional | LOW -- patch versions |

---

## Integration Points

### How Channels Connect to Durable Execution

```
                      Durable Agent Lambda
                     +------------------+
                     |                  |
  agent_handler() -->| ctx.step("llm") |
                     |       |         |
                     | LLM says "need  |
                     | approval"       |
                     |       |         |
                     | channel.send()  |-- reqwest POST --> Slack/Discord/WhatsApp API
                     |       |         |
                     | ctx.wait_for_   |
                     | callback()      |  <-- Lambda SUSPENDS (no compute cost)
                     |       |         |
                     | ... time passes |
                     |       |         |
                     | callback_id     |  <-- SendDurableExecutionCallbackSuccess
                     | resolved        |      (from webhook receiver Lambda)
                     |       |         |
                     | continue agent  |
                     | loop            |
                     +------------------+

  Webhook Receiver Lambda (separate)
  +----------------------------------+
  | API Gateway event (Slack/Discord |
  | webhook POST)                    |
  |       |                          |
  | Verify signature (hmac/sha2)     |
  | Extract message + callback_id    |
  |       |                          |
  | aws-sdk-lambda::Client::         |
  |   send_durable_execution_        |
  |   callback_success()             |
  +----------------------------------+
```

### How Agent Teams Connect

```
  Orchestrator Agent Lambda
  +-----------------------------+
  | Load team config            |
  | For each member:            |
  |   register as MCP tool      |
  |   (DynamicServerManager)    |
  |       |                     |
  | LLM call with team tools    |
  | LLM: "use research-agent"  |
  |       |                     |
  | ctx.invoke() --> member     |
  |   agent Lambda              |
  | (or ctx.parallel for        |
  |  multiple members)          |
  +-----------------------------+
```

### How Scheduled Triggers Connect

```
  EventBridge Scheduler
  +-------------------+
  | cron(0 9 * * *)   |
  | or rate(1 hour)   |
  |       |           |
  | Target: Lambda ARN|
  | Input: AgentRequest JSON
  +-------------------+
         |
         v
  Durable Agent Lambda
  (same binary, same handler)
```

---

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Channel outbound | `reqwest` (raw HTTP) | `slack-morphism` / `serenity` | Platform SDK crates add heavyweight deps for features Lambda doesn't use (Socket Mode, Gateway, Block Kit DSL). Raw HTTP is simpler, already proven in zeroclaw |
| Channel inbound | `wait_for_callback` + webhook Lambda | Polling (like zeroclaw `listen()`) | Lambda cannot poll -- it suspends. `wait_for_callback` is the native Lambda mechanism for external async input. Zero compute cost while waiting |
| Agent Teams | `pmcp` `DynamicServerManager` | Custom tool routing | pmcp already has runtime tool registration with `add_tool()`/`remove_tool()`. No need to build a custom registry |
| Scheduled triggers | `aws-sdk-scheduler` (EventBridge Scheduler) | `aws-sdk-eventbridge` (EventBridge rules) | Scheduler is purpose-built for time-based triggers. Rules are for event pattern matching. Scheduler supports one-time schedules (useful for "run agent at 3pm tomorrow") which Rules do not |
| Scheduled triggers | SAM `AWS::Scheduler::Schedule` | CloudWatch Events cron | EventBridge Scheduler supersedes CloudWatch Events. More features, same pricing |
| Webhook receiver | Separate Lambda + API Gateway | API Gateway direct integration | Need to verify webhook signatures (HMAC for Slack, Ed25519 for Discord) and extract callback_id from message metadata. Too complex for API Gateway mapping templates |
| Discord integration | REST API + webhooks | Gateway bot (WebSocket) | Lambda suspends between invocations -- cannot maintain WebSocket. REST API + webhooks is the only viable pattern for serverless Discord bots |
| Webhook signature | `hmac` + `sha2` (RustCrypto) | `ring` | `sha2` already in project deps. `hmac` is the natural companion from RustCrypto ecosystem. `ring` would add a parallel crypto implementation |

---

## Sources

- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/Cargo.toml` -- current agent dependencies (lockfile-verified)
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/Cargo.lock` -- resolved dependency versions
- `/Users/guy/projects/LocalAgent/zeroclaw/src/channels/` -- reference channel implementations (traits.rs, slack.rs, discord.rs, whatsapp.rs, activity.rs)
- `/Users/guy/projects/LocalAgent/zeroclaw/Cargo.toml` -- zeroclaw dependency versions
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/server/dynamic.rs` -- pmcp DynamicServerManager API
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/server/builder.rs` -- pmcp ServerCoreBuilder API
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml` -- pmcp v2.0.2 features and deps
- `/Users/guy/Development/mcp/sdk/pmcp-run/amplify/data/resource.ts` -- pmcp-run data schema
- `/Users/guy/Development/mcp/sdk/pmcp-run/components/dashboard/authentic-navigation.tsx` -- pmcp-run navigation
- [crates.io: aws-sdk-scheduler](https://crates.io/crates/aws-sdk-scheduler) -- v1.97.0, verified 2026-03-24
- [crates.io: aws-sdk-eventbridge](https://crates.io/crates/aws-sdk-eventbridge) -- v1.104.0, verified but not recommended
- [crates.io: reqwest](https://crates.io/crates/reqwest) -- v0.13.2, verified 2026-03-24
- [crates.io: pmcp](https://crates.io/crates/pmcp) -- v2.0.2, verified 2026-03-24
- [crates.io: lambda_http](https://crates.io/crates/lambda_http) -- v1.1.2, verified 2026-03-24
- [crates.io: hmac](https://crates.io/crates/hmac) -- v0.12.1 (stable), verified 2026-03-24
- [crates.io: tokio-tungstenite](https://crates.io/crates/tokio-tungstenite) -- v0.29.0, verified but not recommended
- [crates.io: slack-morphism](https://crates.io/crates/slack-morphism) -- v2.19.0, verified but not recommended
- [AWS Lambda Durable Functions docs](https://docs.aws.amazon.com/lambda/latest/dg/durable-functions.html) -- callback API
- [SendDurableExecutionCallbackSuccess API](https://docs.aws.amazon.com/lambda/latest/api/API_SendDurableExecutionCallbackSuccess.html) -- callback completion
- [EventBridge Scheduler docs](https://docs.aws.amazon.com/scheduler/latest/UserGuide/getting-started.html) -- scheduling Lambda targets
