# Domain Pitfalls

**Domain:** Platform Integration -- Channels, Agent Teams, pmcp-run Agents Tab, PMCP SDK Example
**Researched:** 2026-03-24
**Confidence:** HIGH (codebase analysis, SDK internals, API documentation, published integration patterns)
**Scope:** Pitfalls specific to ADDING these features to an existing, production-quality Durable Lambda MCP Agent

## Critical Pitfalls

Mistakes that cause rewrites, data loss, or architectural dead ends.

### Pitfall 1: WebSocket/Persistent Connection from Lambda (Impossible Architecture)

**What goes wrong:** The channels abstraction needs to receive inbound messages from Slack, Discord, and WhatsApp. The instinctive design is to have the Lambda maintain a WebSocket or SSE connection to receive channel events. This is impossible. Lambda functions are invocation-scoped -- there is no persistent process to hold a WebSocket open. Even with durable execution, the Lambda suspends between invocations and all TCP connections die.

**Why it happens:** Developers reason "the agent needs to listen on Slack" and reach for the Slack RTM (Real Time Messaging) WebSocket API or Discord Gateway. These require a long-lived process. Lambda fundamentally cannot be a long-lived process. Durable execution suspends the Lambda (no compute cost), but suspension kills all connections.

**Consequences:**
- Attempting to use Slack RTM or Discord Gateway from Lambda will fail silently (connection drops on suspend) or noisily (timeout errors on every resume)
- Months of work building a WebSocket-based channel layer that cannot work in the target runtime
- Forced architecture rewrite to a webhook-based model after discovering the constraint

**Prevention:**
- **Channels MUST use webhook/HTTP-push models, never pull/subscription models.** Slack Events API (HTTP POST to your endpoint), Discord Interactions (HTTP POST webhook), WhatsApp Cloud API (webhook notifications) -- all push to you via HTTP
- The Lambda itself receives callbacks via `ctx.wait_for_callback()`. The channel webhook handler is a SEPARATE service (API Gateway + thin Lambda or Amplify function) that receives the channel event and calls `SendDurableExecutionCallbackSuccessCommand` to resume the agent
- For inter-agent communication ("local agent" channel), use `ctx.invoke()` which is natively supported by the durable SDK
- Document this constraint prominently: "Channels are webhook receivers, not connection holders"

**Detection:**
- Any code importing Slack RTM, Discord Gateway, or WebSocket client libraries into the agent Lambda
- Architecture diagrams showing bidirectional connections from the agent Lambda to channel APIs
- `tokio::spawn` for background listeners inside the durable handler

**Phase mapping:** Phase 1 (Channels architecture). Must be the FIRST decision made.

---

### Pitfall 2: Channel Webhook 3-Second Timeout vs. Agent Processing Time

**What goes wrong:** Slack requires a 200 response within 3 seconds of receiving a slash command or interaction event. Discord requires acknowledgment within 3 seconds for interaction webhooks. WhatsApp webhooks should respond with 200 within 30 seconds. The agent Lambda takes 2-30+ seconds to process (LLM call alone is 1-5 seconds). If the channel webhook handler IS the agent Lambda (or synchronously waits for it), every interaction times out.

**Why it happens:** The natural design is: "Slack sends event -> agent processes -> agent responds." But Slack's 3-second timeout means the agent cannot process inline. This is compounded by Lambda cold starts (500ms-2s for Rust, longer with MCP connection setup) which eat into the 3-second budget even before any LLM work begins.

**Consequences:**
- Slack shows "This app didn't respond in time" to users
- Discord drops the interaction and the user sees nothing
- Retries from channel APIs cause duplicate agent invocations
- Users perceive the agent as broken

**Prevention:**
- **Three-service architecture for each channel:**
  1. **Webhook Receiver** (API Gateway + thin Lambda): Validates the webhook signature, returns 200 immediately, enqueues the event (SQS or direct async invoke)
  2. **Agent Lambda** (Durable): Processes the request asynchronously via `wait_for_callback` pattern
  3. **Response Poster** (inside agent step): Posts the response back to the channel API when ready
- The webhook receiver MUST be separate from the agent Lambda. It acknowledges within <500ms, then triggers the agent asynchronously
- For Slack specifically: use `response_url` (valid for 30 minutes) to post delayed responses
- For Discord: use the "deferred response" interaction type (type 5) to acknowledge, then follow up with the actual response via webhook edit
- Store the channel's `response_url` / interaction token as part of the callback payload so the agent can respond when done

**Detection:**
- Timeout errors in channel API dashboards
- Agent Lambda configured as the direct webhook target (no intermediate service)
- Missing `response_url` or `interaction_token` in the agent's event payload

**Phase mapping:** Phase 1 (Channels architecture). The three-service pattern must be established before any channel implementation.

---

### Pitfall 3: Checkpoint Size Explosion with Multi-Agent Team Conversations

**What goes wrong:** Agent Teams means an orchestrator agent delegates tasks to specialist agents. Each agent has its own conversation history. The orchestrator must checkpoint its conversation (which includes tool_use/tool_result pairs where each "tool result" is the FULL response from a specialist agent). A specialist agent's response could be 5-30KB of text. An orchestrator managing 3 specialists across 5 iterations could accumulate 150-450KB of checkpoint data in tool results alone -- dangerously close to the 750KB batch limit, and individual tool results can exceed the 256KB per-operation limit.

**Why it happens:**
- The orchestrator treats each specialist agent as a "tool" (via dynamic MCP server)
- Each specialist returns its full response as the tool result
- The orchestrator's LLM sees these full results and includes them in its context
- Each LLM response from the orchestrator includes reasoning about ALL specialist outputs
- The `IterationResult` checkpoint includes the full `LLMResponse` which includes the full message content
- Multiple levels of nesting: orchestrator step -> specialist invocation -> specialist's own steps

**Consequences:**
- `TerminationReason::CheckpointFailed` once the orchestrator's conversation grows too large
- Team operations that work with 2 agents fail with 4 agents
- Silent data truncation if custom Serdes compresses without careful limits
- Orchestrator cannot resume from checkpoint, entire team execution fails

**Prevention:**
- **Summarize specialist responses before checkpointing.** The orchestrator should receive a summary (not full response) from each specialist. The dynamic MCP server wrapping each agent should truncate or summarize the response to a configurable limit (e.g., first 10KB)
- **Use `run_in_child_context` for each specialist invocation** so that if the specialist result exceeds 256KB, the SDK falls back to `replay_children: true` (replaying the child's operations instead of storing the full result). This is already implemented in `child/execute.rs` lines 96-101
- **Monitor cumulative checkpoint size** across the orchestrator's conversation. Add a pre-check before each LLM call step: estimate the total checkpoint size and warn/truncate if approaching limits
- **Set aggressive `max_iterations` for team orchestrators** (e.g., 5-8, not 25) since each iteration involves multiple specialist calls
- **Consider S3-backed overflow** for large specialist responses: store in S3, checkpoint only the S3 key

**Detection:**
- `checkpoint batch failed` errors in CloudWatch logs for orchestrator Lambda
- Team executions succeeding with simple tasks but failing with complex multi-step tasks
- Orchestrator step data growing monotonically across iterations

**Phase mapping:** Phase 2 (Agent Teams). Data model for team communication must be designed before implementation.

---

### Pitfall 4: Circular Agent Delegation Creates Infinite Durable Execution Loops

**What goes wrong:** Agent A delegates to Agent B (via `ctx.invoke()` or dynamic MCP tool call). Agent B, in its own reasoning, decides to delegate back to Agent A. Agent A is now re-invoked and delegates to Agent B again. This creates an infinite loop, but unlike a simple code loop, each delegation is a durable checkpoint -- the loop is persisted and will resume on every Lambda invocation. There is no natural termination because each agent independently decides to delegate, and the orchestrator's `max_iterations` only counts its own iterations, not the total delegation depth.

**Why it happens:**
- Agent Teams use dynamic MCP servers where each agent is exposed as a tool to other agents
- The LLM decides when to use tools, including the "delegate to Agent X" tool
- Without explicit constraints, nothing prevents Agent A from calling Agent B which calls Agent A
- Each delegation is a durable `ctx.invoke()` or `ctx.step()` -- checkpointed, persistent, and will replay
- The SDK's `max_iterations` guard only limits iterations within a single agent, not across the team

**Consequences:**
- Runaway costs: each delegation cycle involves multiple LLM calls
- Checkpoint data grows unboundedly across delegations
- The execution never completes -- it keeps suspending, resuming, and re-delegating
- Manual intervention required to cancel the durable execution
- In worst case: each delegation creates new durable executions that also loop, creating a tree of runaway executions

**Prevention:**
- **Global delegation depth counter.** Pass a `delegation_depth` field in the agent request. Each agent increments it before delegating. Refuse delegation when depth exceeds a configurable limit (e.g., 3-5)
- **Delegation graph tracking.** Maintain a `visited_agents: Vec<String>` in the delegation context. Before delegating to Agent X, check if X is already in the visited set. If so, return an error to the LLM ("Cannot delegate to Agent X: circular delegation detected")
- **Explicit delegation rules in agent config.** Each agent's registry entry specifies which agents it can delegate to (allowlist). The orchestrator can delegate to specialists, but specialists cannot delegate back to the orchestrator
- **Time-bounded team execution.** Set `DurableConfig.ExecutionTimeout` on the orchestrator Lambda. If the entire team operation exceeds the timeout, the durable execution expires regardless of delegation state
- **Never expose the orchestrator as a tool to its own specialists.** The dynamic MCP server for Agent A should not be available to agents that Agent A delegates to

**Detection:**
- Durable executions with abnormally high operation counts (100+ operations)
- Same agent ARN appearing multiple times in `ctx.invoke()` chains
- Costs spiking without corresponding useful output
- Durable executions hitting `ExecutionTimeout`

**Phase mapping:** Phase 2 (Agent Teams). Delegation constraints must be in the architecture, not just agent prompts.

---

### Pitfall 5: DynamoDB Schema Divergence Across Three Independent Codebases

**What goes wrong:** Three systems share DynamoDB tables (primarily AgentRegistry): the Step Functions Agent (CDK Python), pmcp-run (Amplify Gen 2 TypeScript), and the Durable Agent (SAM Rust). Each codebase has its own type definitions for the DynamoDB schema. When one system adds a field (e.g., `mcp_servers`, `channels`, `team_config`), the other systems silently ignore it -- until they don't. A pmcp-run UI update that changes the field format (e.g., `mcp_servers` from string array to object array) breaks the Durable Agent's deserialization.

**Why it happens:**
- No single source of truth for the DynamoDB schema. Each codebase defines its own types independently
- Amplify Gen 2 generates DynamoDB schema from TypeScript data models. CDK Python generates it from Python. The Rust agent reads it with hand-written `serde` types
- Fields added by one system are unknown to others -- DynamoDB is schemaless, so no validation catches this
- Version skew between deployments: pmcp-run deploys new schema Monday, Durable Agent deploys old reader Tuesday
- Different serialization conventions: Amplify might store `mcpServers` (camelCase), CDK might store `mcp_servers` (snake_case), Rust might expect either

**Consequences:**
- Agent config loading fails with deserialization errors after a pmcp-run deployment
- Step Functions agents break when Durable Agent adds fields they don't expect in query responses
- Data corruption: one system writes a field in format X, another reads it expecting format Y
- Silent data loss: fields added by one system are dropped when another system updates the same item

**Prevention:**
- **Define a canonical schema document** (JSON Schema or Protocol Buffers) that all three codebases reference. Generate types from this shared schema where possible
- **Use additive-only schema changes.** Never rename or change the type of existing fields. Add new fields alongside old ones. Deprecate old fields gradually
- **All readers MUST use `#[serde(deny_unknown_fields)]` sparingly** -- instead, use `#[serde(default)]` on optional fields so unknown fields are tolerated
- **Version the schema.** Add a `schema_version: u32` field to each item type. Readers check the version and handle unknown versions gracefully (read what they can, ignore what they can't)
- **Integration test:** A CI test that deserializes a "golden" DynamoDB item (exported from the real table) through all three systems' types. If any system fails to deserialize, the test fails
- **Strictly separate concerns.** The Durable Agent reads a subset of fields; pmcp-run manages a superset. The agent should never write fields it doesn't own. Use a `managed_by` convention

**Detection:**
- `serde` deserialization panics or errors in agent Lambda logs after a pmcp-run deployment
- Fields mysteriously disappearing from DynamoDB items (overwritten by a system that doesn't know about them)
- Different dashboards (Step Functions admin UI vs pmcp-run Agents tab) showing different data for the same agent

**Phase mapping:** Phase 3 (pmcp-run integration). Must be addressed before any cross-system data sharing.

---

### Pitfall 6: Channel Security Model Defaults to Overpermissive

**What goes wrong:** Channels are added with a "make it work first, secure it later" approach. Every agent gets access to every channel. A Slack channel intended for the "finance" agent is also accessible to the "code review" agent. An agent with access to a WhatsApp channel can send messages to arbitrary phone numbers. The channel's API credentials (Slack bot token, Discord bot token) are shared across all agents, meaning any agent compromise exposes all channels.

**Why it happens:**
- Fastest path to a working demo is a single set of channel credentials shared across agents
- The agent config doesn't specify which channels are allowed, so the channel abstraction defaults to "all channels available"
- zeroclaw's deny-by-default model is referenced in PROJECT.md but not enforced in the initial implementation
- Channel API tokens are stored once in Secrets Manager and passed to all agent invocations

**Consequences:**
- An agent sending messages to unintended channels (Slack channels, Discord servers, WhatsApp contacts)
- API credential exposure: compromise of one agent's Lambda exposes all channel tokens
- Audit trail confusion: which agent sent which message through which channel?
- Regulatory risk: WhatsApp messages sent to wrong recipients, PII exposure through wrong channels

**Prevention:**
- **Deny-by-default from day one.** Each agent's registry entry explicitly lists allowed channels: `channels: [{ type: "slack", channel_id: "C12345", permissions: ["read", "write"] }]`. If no channels are configured, the agent cannot use any channels
- **Per-channel, per-agent credentials.** Each agent gets its own Slack bot token (or uses a shared bot with channel-specific permissions). Store credentials per-agent in Secrets Manager: `agents/{agent_name}/channels/slack`
- **Channel scope validation in the channel abstraction.** Before sending a message, the channel implementation checks: "Is this agent allowed to send to this channel?" If not, return an error to the LLM
- **Audit logging on every channel operation.** Every message sent/received through a channel is logged with: agent_name, channel_type, channel_id, direction, timestamp, message_id. This is not optional
- **Follow zeroclaw's trait pattern:** The `Channel` trait should include an `is_authorized(agent_id, action) -> bool` method that the trait implementor must satisfy

**Detection:**
- Agent logs showing channel operations without corresponding authorization checks
- Single Secrets Manager path used for all agents' channel credentials
- Missing `channels` field in AgentRegistry items (implies all-access default)

**Phase mapping:** Phase 1 (Channels architecture). Security model must be designed alongside the channel abstraction, not retrofitted.

## Moderate Pitfalls

### Pitfall 7: Replay Non-Determinism from Channel State

**What goes wrong:** The agent uses channels to interact with users mid-conversation (e.g., "ask for clarification via Slack"). The channel interaction introduces external state: the user's response text, timing, and whether they responded at all. On replay, the `wait_for_callback` result is cached, so the user's response is deterministically replayed. **But** if the agent code checks channel state OUTSIDE a durable step (e.g., "is the Slack channel still active?", "how many unread messages?"), that check returns different results on replay, potentially causing non-deterministic branching.

**Why it happens:**
- Channel status checks (is-connected, message-count, channel-metadata) feel like reads, not side effects
- Developers put these checks outside `ctx.step()` because they seem lightweight
- But channel state changes between invocations: a Slack channel could be archived, a Discord server could have new members, a webhook URL could expire

**Prevention:**
- **All channel interactions must go through `ctx.wait_for_callback()` or `ctx.step()`.** No channel API calls outside durable operations
- **Channel metadata (channel name, member list, permissions) should be cached in a durable step** if the agent needs to reason about it
- **The channel abstraction should not expose "query" methods** that return live state. Only expose `send(message)` (inside step) and `receive(callback_id)` (via wait_for_callback)
- **Lint rule or code review checklist:** "Is there any channel API call outside a `ctx.step()` or `ctx.wait_for_callback()`?"

**Detection:**
- Agent behavior differing between first execution and replay
- Channel-related data appearing in logs during replay (should be silent during replay)
- Intermittent agent failures that correlate with channel state changes (channel archived, permissions changed)

**Phase mapping:** Phase 1 (Channels architecture). The channel trait must enforce durable-safe usage by design.

---

### Pitfall 8: Callback Token Expiry and Channel Response URL Lifetime Mismatch

**What goes wrong:** The agent sends a message to Slack asking for approval and suspends via `wait_for_callback()`. The callback has a timeout of 24 hours. But Slack's `response_url` (used to post follow-up messages) expires after 30 minutes. Discord interaction tokens expire after 15 minutes. WhatsApp session windows expire after 24 hours. When the user eventually responds (e.g., 2 hours later), the agent resumes but can no longer post to the channel because the response URL/token has expired.

**Why it happens:**
- Each channel API has different token/URL lifetimes, none of which align with the durable callback timeout
- The `wait_for_callback` timeout is set based on business requirements ("wait 24 hours for approval"), not channel API constraints
- Response URLs and interaction tokens are stored in the callback data when the callback is created, but they're expired by the time the callback completes

**Consequences:**
- Agent resumes after callback but cannot respond to the user on the same channel thread
- User sees their approval was received (via webhook acknowledgment) but no confirmation from the agent
- Agent errors trying to post to expired URLs, potentially crashing the execution

**Prevention:**
- **Separate the callback token from the channel response mechanism.** The callback payload should contain the channel ID and thread ID, not the ephemeral response_url. When the agent resumes, it uses the channel API directly (via bot token, not response_url) to post in the same thread
- **For Slack:** Use the bot token + `chat.postMessage` with `thread_ts` instead of `response_url`. The bot token doesn't expire (until revoked)
- **For Discord:** Use the bot token + channel webhook instead of interaction tokens. Create a persistent webhook for the channel
- **For WhatsApp:** Use the 24-hour conversation window. If the window expires, the agent must send a template message to re-open it
- **Document channel-specific lifetime constraints** in the channel abstraction:
  - Slack `response_url`: 30 minutes
  - Discord interaction token: 15 minutes
  - WhatsApp session window: 24 hours
  - Durable callback: configurable (up to `ExecutionTimeout`)

**Detection:**
- 410 Gone or 404 errors when agent tries to post channel responses after resuming from callback
- Agent completing successfully but user never receiving the response
- Mismatch between `CallbackConfig.timeout` and channel token lifetimes in config

**Phase mapping:** Phase 1 (Channels implementation). Must be addressed per-channel, not as a generic pattern.

---

### Pitfall 9: Dynamic MCP Server Deployment Latency Stalls Team Operations

**What goes wrong:** Agent Teams work by dynamically deploying an MCP server for each specialist agent (so the orchestrator can `call_tool` on it). If the MCP server is a Lambda behind an API Gateway, there's a cold start delay. If the MCP server is provisioned on-demand via pmcp-run's deployment pipeline, the deployment itself takes 30-120 seconds. The orchestrator calls `list_tools()` on the specialist's MCP server, but the server isn't ready yet. Connection timeout. Retry. Still not ready. The orchestrator's iteration burns through retries waiting for infrastructure.

**Why it happens:**
- The "agents as MCP tools" model assumes MCP servers are always running and discoverable
- pmcp-run's deployment pipeline (build, push to registry, deploy to Lambda) is designed for human-initiated deployments, not real-time agent team formation
- Lambda cold starts for MCP servers compound: orchestrator cold start + specialist agent cold start + specialist's MCP server cold start = 5-15 seconds before the first tool call
- Dynamic server generation means the MCP server URL isn't known until deployment completes

**Consequences:**
- Team operations take 30-120+ seconds just to start (before any LLM work)
- Transient failures if the deployment pipeline has errors
- Orchestrator retries burn through callback/step retry budgets waiting for infrastructure
- Complex error diagnosis: "Is the specialist agent broken, or is the MCP server not deployed yet?"

**Prevention:**
- **Pre-deploy specialist MCP servers.** When an agent is registered as a team member, its MCP server should be deployed then (admin-time, not runtime). The orchestrator connects to pre-existing URLs, not dynamically provisioned ones
- **Warm pools.** If dynamic deployment is required, maintain a pool of pre-warmed generic MCP server Lambdas that can be configured at runtime (route to the right specialist agent based on headers/path)
- **Health checks before orchestration.** The orchestrator's first step should be `ctx.step("health-check-team")` which pings all specialist MCP servers and fails fast if any are unreachable
- **Use `ctx.invoke()` instead of MCP for agent-to-agent communication.** The durable SDK's `invoke` natively calls other Lambda functions and handles retry/checkpoint. This bypasses the MCP server layer entirely for inter-agent calls, using MCP only for external tool access
- **Fallback timeout.** If a specialist MCP server doesn't respond within 10 seconds, degrade gracefully (skip that specialist, inform the orchestrator LLM, continue with available specialists)

**Detection:**
- Team operations timing out during the `discover-tools` step for specialist servers
- Long gaps in CloudWatch logs between orchestrator start and first LLM call
- Deployment pipeline invocations correlated with team operation requests

**Phase mapping:** Phase 2 (Agent Teams). Architecture must decide "deploy at registration time" vs "deploy at runtime" early.

---

### Pitfall 10: Breaking Existing pmcp-run MCP Hosting Features

**What goes wrong:** Adding the Agents tab to pmcp-run requires changes to the Amplify Gen 2 data model (new Agent entity), the Next.js frontend (new pages/components), and potentially the deployment pipeline (agents deploy differently from MCP servers). These changes inadvertently break existing MCP server hosting: a shared component is modified, a database migration drops or renames a field, the deployment pipeline's IAM role gains agent permissions that conflict with MCP server permissions, or the LCARS UI navigation changes confuse existing users.

**Why it happens:**
- pmcp-run is a production system with existing users hosting MCP servers
- The Amplify Gen 2 data model is declarative -- adding a new model can trigger schema migrations that affect existing models if relationships are added carelessly
- Frontend components may be shared (e.g., "deployment status" component used by both servers and agents)
- The deployment pipeline may be shared infrastructure (same CodeBuild project, same IAM roles)

**Consequences:**
- Existing MCP servers stop deploying or become unreachable
- Users' existing UI workflows break (navigation, forms, dashboards)
- Data corruption if a schema migration goes wrong
- Loss of trust: "I can't use pmcp-run for MCP servers anymore because the agent stuff broke it"

**Prevention:**
- **Additive only.** New tables/entities for agents, not modifications to existing server entities. No shared DynamoDB tables between agents and MCP servers
- **Feature flag the Agents tab.** Ship the backend changes first (hidden behind a flag), validate they don't affect existing features, then enable the UI
- **Separate IAM roles.** Agent deployment pipeline uses its own IAM role, not the MCP server deployment role
- **Amplify Gen 2 data model isolation.** Define `Agent`, `AgentExecution`, `AgentTeam` as new models with no relationships to `McpServer`, `Deployment`, etc. Cross-referencing is done by convention (same name string), not by Amplify relationship
- **Regression test suite.** Before merging any pmcp-run changes, run the existing MCP server deployment and hosting tests. Fail the PR if any existing tests break
- **Staged rollout.** Deploy agent backend to a staging environment, test all existing MCP server operations, then promote to production

**Detection:**
- Existing MCP server tests failing after agent-related changes
- Users reporting MCP server hosting issues correlated with agent feature deployments
- Amplify Gen 2 `amplify push` failing or producing unexpected schema migrations

**Phase mapping:** Phase 3 (pmcp-run integration). Every pmcp-run change must be validated against existing functionality.

---

### Pitfall 11: Amplify Gen 2 Schema Compatibility Across Separate Repos

**What goes wrong:** The Step Functions Agent uses Amplify Gen 2 in one repo. pmcp-run uses Amplify Gen 2 in another repo. Both define data models that reference the same conceptual entities (Agent, AgentRegistry). Amplify Gen 2 generates DynamoDB tables per-app -- so the same "Agent" concept exists in two different DynamoDB tables with two different schemas in two different AWS accounts (or the same account with different stack names). Sharing data between them requires explicit cross-referencing, and changes to one schema don't propagate to the other.

**Why it happens:**
- Amplify Gen 2's `defineData()` is scoped to a single app/backend. There is no built-in mechanism for sharing data models across repos
- The recommended pattern (TypeScript path aliases in tsconfig.json) works for type sharing within a monorepo but not across separate repos with separate deployments
- Each Amplify app creates its own DynamoDB tables with app-specific prefixes
- The Durable Agent reads from DynamoDB directly (not through Amplify's generated API), so it must know the exact table names and schema

**Consequences:**
- Data duplication: same agent config exists in two DynamoDB tables, potentially out of sync
- Schema drift: one repo updates the Agent model, the other uses the old version
- Confusion about which table is the source of truth for agent configuration
- Migration complexity: moving from Step Functions to Durable Lambda requires data migration between tables

**Prevention:**
- **Designate ONE canonical DynamoDB table for each entity.** AgentRegistry lives in ONE table, owned by ONE system (pmcp-run is the natural owner as the unified platform). Other systems read from it, never write to it (or write through a shared API)
- **The Durable Agent reads table names from environment variables** (e.g., `AGENT_REGISTRY_TABLE`), not from Amplify-generated constants. This decouples the agent from the Amplify app that owns the table
- **Share types via a shared package, not Amplify data model.** Create a `@pmcp/agent-types` npm package (or Rust crate for agent types) that defines the canonical schema. Both repos depend on this package
- **Use Amplify's `exportedTable` or `addOutput`** to expose table ARNs/names as CloudFormation exports, so other stacks can reference them
- **Migration strategy:** When moving management from Step Functions to pmcp-run, use a one-time data migration script, not ongoing sync. Pick a cutover date

**Detection:**
- Same entity name appearing in multiple Amplify Gen 2 `schema.ts` files across repos
- Queries to wrong DynamoDB tables (table name mismatch between environments)
- Data inconsistencies when viewing the same agent in different UIs

**Phase mapping:** Phase 3 (pmcp-run integration). Must be resolved before building the Agents tab.

---

### Pitfall 12: IAM Permission Sprawl Across Services

**What goes wrong:** The Durable Agent Lambda needs IAM permissions for: DynamoDB (AgentRegistry read), Secrets Manager (API keys), Lambda checkpoint API, Lambda invoke (for agent teams), MCP server endpoints (if behind IAM auth), and now channel APIs (SQS for channel events, SNS for notifications, API Gateway for webhooks, Secrets Manager for channel tokens). Each new feature adds permissions. The agent's IAM role grows into a god-role with broad access across services. A bug or prompt injection in the agent could exploit these permissions.

**Why it happens:**
- Each feature is developed independently and adds its own IAM permissions
- SAM templates tend to use `Resource: "*"` for convenience (already visible in the current template)
- Channels each need their own credentials stored in Secrets Manager
- Agent Teams need `lambda:InvokeFunction` on other agent Lambdas
- No one reviews the cumulative IAM policy across all features

**Consequences:**
- Over-permissioned Lambda: a compromised agent can access resources beyond its needs
- IAM policy size limits: Lambda execution role policies have a 10,240 character limit
- Blast radius: a bug in channel handling could accidentally invoke other Lambdas or read wrong secrets
- Audit findings: security reviews flag the broad permissions

**Prevention:**
- **Scope all resources.** Replace `Resource: "*"` with specific ARNs. `lambda:CheckpointDurableExecution` should be scoped to the agent's own ARN. `dynamodb:GetItem` should be scoped to the AgentRegistry table ARN. `secretsmanager:GetSecretValue` should be scoped to the agent's secret ARN pattern
- **Separate IAM roles per concern.** Consider using IAM role assumption: the agent's base role has minimal permissions; when it needs channel access, it assumes a channel-specific role with just the channel permissions
- **Permission boundaries.** Set IAM permission boundaries on the agent role that cap the maximum permissions regardless of policy additions
- **Regular audit.** Run `IAM Access Analyzer` on the agent's execution role after each phase. Generate least-privilege policies from CloudTrail logs
- **SAM template review gate.** Every PR that modifies the SAM template's `Policies` section requires a security review

**Detection:**
- `Resource: "*"` in production SAM templates
- Single IAM role with 20+ distinct action types
- IAM Access Analyzer findings for the agent role
- Lambda execution role policy approaching size limits

**Phase mapping:** All phases. Each phase should tighten permissions for its features before moving on.

## Minor Pitfalls

### Pitfall 13: Channel Rate Limit Violations Under Agent Load

**What goes wrong:** An agent team with 3 specialists all posting to the same Slack channel simultaneously hits Slack's rate limit (1 message/second for `chat.postMessage`, or the stricter limits for non-Marketplace apps since May 2025). Discord has similar rate limits per channel. The channel abstraction doesn't track rate limits, causing sporadic 429 errors that the agent interprets as failures.

**Prevention:**
- **Per-channel rate limiter in the channel abstraction.** A simple token bucket per channel_id, enforced before calling the channel API. Wait (via `ctx.wait()`) if rate limited
- **Slack-specific:** Respect `Retry-After` headers. Slack returns them on 429 responses. The channel implementation should pause and retry, not propagate the error to the agent
- **Discord-specific:** Use the `X-RateLimit-*` headers to track remaining budget per channel
- **WhatsApp-specific:** Template message limits are per-phone-number per 24 hours. Track usage and fail gracefully before hitting the limit
- **Batch messages when possible.** Instead of sending 5 separate messages for 5 tool results, combine them into one formatted message

**Phase mapping:** Phase 1 (Channel implementation). Rate limiting must be built into the channel abstraction, not added later.

---

### Pitfall 14: Secret Management Fragmentation

**What goes wrong:** The system now has secrets scattered across multiple paths: LLM API keys (`agents/*/api_key`), MCP server credentials, Slack bot tokens, Discord bot tokens, WhatsApp API keys. Each stored in Secrets Manager under different path conventions by different teams. The Durable Agent needs to read all of them. The pmcp-run backend needs some of them. The Step Functions agent needs its own subset. No consistent naming or access pattern.

**Prevention:**
- **Canonical secret path convention:** `/{service}/{resource_type}/{resource_name}/{secret_name}`. Example: `/pmcp/agents/finance-agent/slack_token`, `/pmcp/agents/finance-agent/anthropic_key`
- **One Secrets Manager read per agent invocation.** Load all needed secrets in a single `ctx.step("load-secrets")` at agent start, not scattered across the handler. Cache for the invocation lifetime
- **Never checkpoint secrets.** The secret values must not appear in checkpoint data. Load them outside durable steps or use a pattern where the step loads a reference, and the actual secret is fetched outside the step
- **Rotation-safe.** The agent's secret loading should always fetch the current version, not a cached version from a previous invocation. Since `load-secrets` is a durable step, the secrets are fixed for one execution but fresh for the next

**Phase mapping:** Phase 1 (Channels architecture). Secret paths must be decided before any channel implementation stores credentials.

---

### Pitfall 15: Message History Grows Unboundedly with Channel Interactions

**What goes wrong:** Each channel interaction adds messages to the agent's conversation history: the user's channel message becomes a tool result, the agent's response becomes an assistant message, follow-up questions add more messages. In a human-in-the-loop pattern with 10 back-and-forth exchanges via Slack, the message history grows by 20+ messages (10 user messages as tool results + 10 assistant responses). Combined with the existing agent loop messages, this pushes toward both the LLM context window limit and the checkpoint size limit.

**Prevention:**
- **Separate channel conversation from agent reasoning.** The channel interaction should be a single `wait_for_callback()` that returns the user's response as a concise string. The full Slack thread history should NOT be injected into the agent's message history
- **Summarize long channel interactions.** If a channel interaction involves multiple back-and-forth messages (e.g., clarification dialog), summarize the outcome into a single tool result: "User approved the budget of $50K after requesting the breakdown"
- **Context window monitoring.** Before each LLM call, check cumulative token count against the model's context limit. If approaching the limit, summarize older messages
- **Channel interaction budget.** Set a maximum number of channel interactions per agent execution (e.g., 5). After that, the agent must complete without further human input

**Phase mapping:** Phase 1 (Channels implementation) and Phase 2 (Agent Teams).

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Channels architecture (Phase 1) | WebSocket from Lambda (#1), 3-second timeout (#2), overpermissive security (#6) | Webhook-only model, three-service architecture, deny-by-default from day one |
| Channel implementation (Phase 1) | Replay non-determinism (#7), callback token expiry mismatch (#8), rate limits (#13) | All channel ops inside durable steps, use bot tokens not response_urls, per-channel rate limiter |
| Channel secrets (Phase 1) | Secret fragmentation (#14) | Canonical path convention, single load step, never checkpoint secrets |
| Agent Teams (Phase 2) | Checkpoint explosion (#3), circular delegation (#4), deployment latency (#9) | Summarize specialist responses, global depth counter with visited-set, pre-deploy specialist servers |
| Agent Teams data model (Phase 2) | Message history growth (#15) | Separate channel conversations from reasoning, summarize interactions |
| pmcp-run integration (Phase 3) | Breaking existing features (#10), Amplify schema compatibility (#11), DynamoDB divergence (#5) | Additive only, feature flags, canonical table ownership, regression tests |
| Cross-system IAM (All phases) | Permission sprawl (#12) | Scoped resources, separate roles per concern, regular IAM audit |

## Sources

- SDK source analysis: `src/context/durable_context/callback/execute.rs` -- callback suspension mechanism, `terminate_for_callback()` (HIGH confidence)
- SDK source analysis: `src/context/durable_context/child/execute.rs` lines 96-101 -- `replay_children` overflow handling for >256KB child context results (HIGH confidence)
- SDK source analysis: `src/checkpoint/manager.rs` -- 750KB `MAX_PAYLOAD_SIZE` batch limit (HIGH confidence)
- SDK source analysis: `src/types/config/callback.rs` -- CallbackConfig with timeout and heartbeat_timeout (HIGH confidence)
- Agent binary analysis: `examples/src/bin/mcp_agent/handler.rs` -- current IterationResult checkpoint pattern, child context per iteration (HIGH confidence)
- Agent binary analysis: `examples/src/bin/mcp_agent/mcp/client.rs` -- MCP connection lifecycle, tool routing (HIGH confidence)
- `examples/template.yaml` -- current IAM policy pattern with `Resource: "*"` (HIGH confidence)
- `.planning/PROJECT.md` -- milestone requirements, integration targets, zeroclaw reference (HIGH confidence)
- AWS Lambda durable functions best practices: [AWS docs](https://docs.aws.amazon.com/lambda/latest/dg/durable-best-practices.html) -- 256KB checkpoint limit, S3 overflow pattern (HIGH confidence)
- Slack API rate limits: [Slack docs](https://docs.slack.dev/apis/web-api/rate-limits/) -- 1 msg/sec write limit, 3-second interaction timeout (HIGH confidence)
- Slack rate limit changes May 2025: [Slack changelog](https://docs.slack.dev/changelog/2025/05/29/rate-limit-changes-for-non-marketplace-apps/) -- non-Marketplace app restrictions (HIGH confidence)
- Slack + Lambda durable functions integration pattern: [DEV.to article](https://dev.to/dobeerman/pause-your-lambda-building-a-slack-approval-workflow-with-aws-durable-functions-17jo) -- wait_for_callback with Slack, three-service architecture (MEDIUM confidence)
- Discord message limits: 2,000 characters per message (MEDIUM confidence -- from training data, well-known)
- Discord interaction timeout: 3 seconds for initial acknowledgment (MEDIUM confidence -- from training data)
- WhatsApp Business API: 24-hour conversation windows, webhook response within 30 seconds (MEDIUM confidence -- from [Meta developer docs](https://developers.facebook.com/docs/whatsapp/messaging-limits/))
- Multi-agent coordination strategies: [Galileo blog](https://galileo.ai/blog/multi-agent-coordination-strategies) -- circular delegation prevention, token boundaries, deterministic task allocation (MEDIUM confidence)
- MCP server cold start on Lambda: 3-5 seconds for FastMCP (MEDIUM confidence -- from [DEV.to article](https://dev.to/bytesrack/stop-the-latency-why-mcp-servers-belong-on-dedicated-hardware-not-lambda-functions-169n))
- DynamoDB single-table design pitfalls: schema evolution difficulty, multi-service coordination (MEDIUM confidence -- from [AWS blog](https://aws.amazon.com/blogs/database/single-table-vs-multi-table-design-in-amazon-dynamodb/))
- IAM least privilege for Lambda: [AWS docs](https://docs.aws.amazon.com/lambda/latest/operatorguide/least-privilege-iam.html) -- IAM Access Analyzer, permission boundaries (HIGH confidence)
- Amplify Gen 2 multi-repo patterns: [Amplify docs](https://docs.amplify.aws/react/deploy-and-host/fullstack-branching/mono-and-multi-repos/) -- TypeScript path aliases, no cross-repo data model sharing (MEDIUM confidence)
