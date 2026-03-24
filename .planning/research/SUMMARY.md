# Project Research Summary

**Project:** Durable Lambda MCP Agent Platform v2.0
**Domain:** AI Agent Platform — Channels, Agent Teams, Platform UI, SDK Example
**Researched:** 2026-03-24
**Confidence:** HIGH (primary findings verified against source code; ecosystem patterns from official docs and reference implementations)

## Executive Summary

The v1.0 durable agent (LLM client, MCP integration, agent loop, observability, deployment) is complete and production-ready. This research covers the v2.0 milestone: adding communication channels for human-in-the-loop workflows, agent team orchestration for multi-agent delegation, a management UI in pmcp-run, and packaging the agent as a reference MCP Tasks example. All four capability areas compose cleanly onto the existing durable primitives without replacing them — channels wrap `wait_for_callback()`, agent teams wrap `ctx.invoke()`, and the UI is a control plane layer over the existing AgentRegistry DynamoDB schema.

The single most important architectural constraint is that Lambda is invocation-scoped. Channels cannot be persistent listeners — they can only send outbound messages via REST APIs and receive inbound messages via webhook callbacks that call `SendDurableExecutionCallbackSuccess`. This constraint rules out Slack Socket Mode, Discord Gateway WebSocket, and any polling-based listen loop. The correct architecture is a thin webhook receiver Lambda (separate from the agent) that acknowledges the channel platform within 3 seconds and then asynchronously resumes the suspended durable agent. Every channel design decision flows from this constraint.

The critical risk for Agent Teams is checkpoint size. An orchestrator agent checkpoints each specialist's full response as a tool result. With 3-5 specialists each returning 5-30KB of output, plus multi-iteration orchestrator context, the 750KB per-batch checkpoint limit can be reached. The mitigation is summarize-before-checkpoint: the team layer truncates specialist responses to a configurable limit before the orchestrator's LLM sees them. The risk for the pmcp-run Agents tab is schema divergence: three codebases (Step Functions CDK, pmcp-run Amplify Gen 2, Durable Agent SAM) share DynamoDB table concepts but define their types independently. Additive-only schema changes and a single canonical table ownership model prevent data corruption.

## Key Findings

### Recommended Stack

The v2.0 agent binary needs zero new Cargo dependencies. Channel outbound delivery uses the existing `reqwest` 0.13.2. Channel inbound reception uses the existing `wait_for_callback()` SDK primitive. Agent Teams use the existing `pmcp` `DynamicServerManager`. The only new Rust dependencies are for the webhook receiver binary: `lambda_http` 1.1.2 (API Gateway events in a separate Lambda) and `hmac` 0.12.1 (HMAC-SHA256 verification for Slack and WhatsApp webhooks). For scheduled triggers, `aws-sdk-scheduler` 1.97.0 is added for programmatic schedule management; simple cron schedules can be declared in SAM YAML without code. On the TypeScript side, pmcp-run gains `@aws-sdk/client-scheduler` for the Agents tab's schedule management Amplify function.

**Core technologies (v2.0 additions only):**
- `lambda_http` 1.1.2 — webhook receiver Lambda (API Gateway HTTP events); same version family as `lambda_runtime` already in use
- `hmac` 0.12.1 — HMAC-SHA256 webhook signature verification (Slack, WhatsApp); RustCrypto, pairs with `sha2` already present as a transitive dep
- `aws-sdk-scheduler` ~1.97 — EventBridge Scheduler for programmatic schedule CRUD; purpose-built for scheduling (vs. EventBridge rules which are for event routing, not scheduling)
- `@aws-sdk/client-scheduler` ^3.x — schedule management from Amplify Gen 2 functions in pmcp-run

**Explicitly excluded:**
- `slack-morphism`, `serenity`, `tokio-tungstenite` — all require persistent connections incompatible with Lambda
- `aws-sdk-eventbridge` — EventBridge rules are for event routing; Scheduler is the correct service
- `axum` as direct dep — already a transitive dep via pmcp's `streamable-http`; not needed directly in the agent binary

### Expected Features

**Must have (table stakes):**
- Channel trait with `deliver()` + `health_check()` (adapted from zeroclaw; drops `listen()` which is inapplicable in Lambda)
- Channel registry mapping named channels to adapters, configured in AgentRegistry
- Slack channel adapter — highest enterprise value, uses Web API `chat.postMessage`
- Webhook/callback adapter — generic mechanism, maps callback_id to `wait_for_callback()`
- Durable channel send via `ctx.step()` — required for idempotency on replay; prevents duplicate messages
- Durable channel receive via `wait_for_callback()` — the correct Lambda-native async input mechanism
- Tool approval gate — agent pauses, sends approval request via channel, waits; unlocks production use with destructive tools
- Deny-by-default security model — channels must be explicitly allowed per-agent; no implicit access
- Agent-as-MCP-tool wrapping — each team member exposed as synthetic tool with `team://` routing prefix
- Agent team sequential execution via `ctx.step()` — pipeline patterns (research -> draft -> review)
- Agent team parallel execution via `ctx.map()` — fan-out patterns (analyze from 5 perspectives simultaneously)
- Agent list/create/edit/delete UI (LCARS design system) in pmcp-run
- On-demand execution and execution history in pmcp-run

**Should have (differentiators):**
- Discord channel adapter (second most important developer channel)
- Hierarchical team orchestration with model tiering (Opus orchestrator, Sonnet/Haiku workers; 40-60% cost reduction per industry data)
- Scheduled agent execution via EventBridge Scheduler (cron/rate triggers from UI)
- Execution detail view with full conversation history rendering
- MCP Tasks integration — durable agent exposed as MCP server with `TaskSupport::Required`, full task lifecycle

**Defer to v3+:**
- WhatsApp, Signal channels — implement only on concrete user request
- Live execution streaming to UI — high complexity; polling via MCP Tasks `pollInterval` works
- Agent handoff (explicit transfer of control) — OpenAI-style; uncertain value over explicit orchestration
- Custom orchestration DSL for team topologies — recreates Step Functions; the LLM IS the orchestrator

### Architecture Approach

The v2.0 architecture layers three new modules onto the existing `mcp_agent` binary without modifying the core agent loop. Channels integrate as built-in tools injected during tool discovery — the LLM calls `channels__send_<name>` like any MCP tool, the routing handler detects the `channel://` prefix, and the Channel Router handles `deliver()` + `wait_for_callback()`. Agent Teams inject `team://` prefixed tools during discovery; when the LLM calls them, `ctx.invoke()` calls the member agent Lambda, which runs its own independent durable execution. Crucially, all agents share a single Lambda binary — the `agent_name` in the request selects the AgentRegistry config, so no per-agent Lambda deployment is needed. The pmcp-run Agents tab is an additive extension with new Amplify Gen 2 models that have zero relationships to existing McpServer/Deployment models.

**Major components:**
1. **Channel Router** (`mcp_agent/channels/`) — maps named channels to `wait_for_callback()` + platform REST delivery; Slack, Discord, Webhook, and LambdaCallback adapters; deny-by-default security enforcement
2. **Webhook Receiver Lambda** (new binary `channel_webhook_receiver`) — validates HMAC/Ed25519 signatures, returns 200 immediately (<500ms), calls `SendDurableExecutionCallbackSuccess` to resume suspended agent
3. **Agent Team Orchestrator** (`mcp_agent/teams/`) — generates synthetic MCP tool definitions for team members at discovery time, routes `team://` calls to `ctx.invoke()`, enforces delegation depth limits and visited-agent tracking
4. **pmcp-run Agents Tab** — Amplify Gen 2 `AgentConfig`/`AgentExecution` models (additive, isolated), LCARS pages (list, edit, detail, history), Amplify functions for CRUD and EventBridge Scheduler management
5. **PMCP SDK Example** — durable agent binary as `pmcp-tasks`-compatible MCP server; durable states map to MCP Task lifecycle states (running->working, waiting_for_callback->input_required)

### Critical Pitfalls

1. **WebSocket/persistent connection from Lambda (impossible architecture)** — Lambda suspends between invocations; all TCP connections die on suspend. Must use webhook/HTTP-push models exclusively for all channel inbound communication. Detection: any import of Slack RTM, Discord Gateway, or WebSocket client in the agent Lambda.

2. **Channel webhook 3-second timeout vs. agent processing time** — Slack requires 200 within 3 seconds; Discord within 3 seconds; agent processing takes 2-30+ seconds. Must use three-service pattern: thin Webhook Receiver Lambda (acknowledges immediately) + async agent invocation + agent posts response via bot token when done. Store thread_id + channel_id in callback payload, NOT ephemeral `response_url` (Slack's expires in 30 minutes; Discord's interaction token expires in 15 minutes).

3. **Checkpoint size explosion with multi-agent team conversations** — 3-5 specialists returning 5-30KB each, multi-iteration orchestrator loop accumulates 150-450KB of tool results, approaching the 750KB batch limit. Mitigation: summarize specialist responses before checkpointing; use `run_in_child_context` for specialist invocations (SDK falls back to replaying children instead of storing full result); cap orchestrator `max_iterations` at 5-8.

4. **Circular agent delegation creates infinite durable loops** — Agent A delegates to Agent B which delegates back to Agent A; each hop is a durable checkpoint, creating an infinite persistent loop. Mitigation: pass `delegation_depth` counter in `AgentRequest`; enforce maximum depth (3-5); maintain `visited_agents: Vec<String>` set in the request; never expose orchestrator as a tool to its own specialists.

5. **DynamoDB schema divergence across three independent codebases** — Step Functions CDK (Python), pmcp-run Amplify Gen 2 (TypeScript), and Durable Agent SAM (Rust) independently define types for shared DynamoDB tables. Silent corruption when one system writes a field the others don't expect, or renames a field using different case conventions. Mitigation: additive-only changes; `#[serde(default)]` on optional Rust fields; designate pmcp-run as canonical table owner; agent reads table names from environment variables, not hardcoded constants.

6. **Channel security defaults to overpermissive** — Without explicit deny-by-default, every agent gets every channel and every channel credential. Must configure allowed channels per-agent in AgentRegistry from day one; per-agent Secrets Manager paths for channel credentials; audit logging on every channel operation.

## Implications for Roadmap

The dependency chain is clear from research: channels unlock human-in-the-loop (highest immediate value); agent teams depend on stable single-agent execution; the Agents tab UI depends on a stable data model from Phases 1-2; the PMCP SDK example documents the completed platform. The phase order follows the critical path while ensuring the most dangerous architectural decisions (webhook-vs-WebSocket, delegation loops, schema ownership) are locked in before any implementation begins.

### Phase 1: Channels Abstraction and Approval Flow

**Rationale:** Highest immediate business value — human-in-the-loop unlocks production use with destructive tools (deploy, delete, payment). The webhook-vs-WebSocket architectural decision must be made first because it cascades into all channel implementation choices. Security model (deny-by-default) must be designed here, not retrofitted. This phase has no dependency on any other v2.0 work and can start immediately.

**Delivers:** Channel trait (`deliver()` + `health_check()`), Slack and Webhook adapters, webhook receiver Lambda binary, durable send/receive pattern, tool approval gate, deny-by-default security model, AgentRegistry `channels` schema extension

**Addresses features:** Channel trait, channel registry, Slack adapter, webhook adapter, durable channel send/receive, tool approval gate, tool danger classification, approval timeout with default-deny, approval response with modification

**Avoids:** Pitfall 1 (WebSocket), Pitfall 2 (3-second timeout), Pitfall 6 (security defaults), Pitfall 7 (replay non-determinism from channel state), Pitfall 8 (callback token expiry)

**Key decisions to lock in before writing code:**
- Webhook Receiver Lambda is always separate from the agent Lambda; never configure the agent as the direct webhook target
- Callback payload contains `thread_id` + `channel_id` (permanent identifiers), not ephemeral `response_url` or interaction tokens
- Deny-by-default enforced in the channel abstraction layer, not in agent prompts

### Phase 2: Agent Teams

**Rationale:** Depends on stable single-agent execution (v1.0 complete) but not on channels. Can proceed in parallel with Phase 1 if resources allow, but the shared 750KB checkpoint budget means understanding baseline channel overhead first is prudent. Delegation loop prevention must be in the data model before any team code is written — it cannot be added as an afterthought to a running system.

**Delivers:** Agent-as-MCP-tool wrapping, `team://` routing prefix in tool execution handler, `ctx.invoke()`-based delegation, delegation depth guard, visited-agent tracking, sequential and parallel team patterns, team configuration schema in AgentRegistry, single-Lambda-binary multi-agent design

**Addresses features:** Agent-as-MCP-tool, dynamic tool definition generation, sequential execution via `ctx.step()`, parallel execution via `ctx.map()`, hierarchical orchestration, model tiering (existing per-agent config)

**Avoids:** Pitfall 3 (checkpoint explosion — summarize specialist responses), Pitfall 4 (circular delegation — depth counter and visited set), Pitfall 9 (MCP server deployment latency — use `ctx.invoke()` directly for inter-agent calls, not MCP transport)

**Key decisions to lock in before writing code:**
- All agents share one Lambda binary; `agent_name` in request selects AgentRegistry config
- Specialist responses summarized to configurable limit (e.g., 10KB) before checkpointing to orchestrator
- `delegation_depth` and `visited_agents` fields are part of `AgentRequest` from the start, not added later
- Pre-deploy specialist agent configs at registration time; orchestrator connects to pre-existing tool definitions, not dynamically provisioned infrastructure

### Phase 3: pmcp-run Agents Tab

**Rationale:** Requires the stable AgentRegistry schema produced by Phases 1-2 (channels fields, team_config fields). Must not break existing MCP server hosting — additive-only Amplify Gen 2 schema changes with zero relationships to existing models. Canonical DynamoDB table ownership must be decided and implemented before writing any UI that depends on it.

**Delivers:** Agent CRUD UI (LCARS design, ported from Step Functions Agent 12-page pattern), model registry migration from Step Functions, on-demand execution from UI, execution history list, execution detail view with conversation rendering, API key management via Secrets Manager, scheduled execution (EventBridge Scheduler integration)

**Addresses features:** Agent list, create/edit, delete, model selector, MCP server selector, API key management, on-demand execution, execution status tracking, execution history, execution detail, scheduled/cron triggers

**Avoids:** Pitfall 5 (DynamoDB schema divergence — single canonical table, additive-only changes), Pitfall 10 (breaking existing pmcp-run MCP hosting features — additive models, feature-flagged rollout, regression tests before merge), Pitfall 11 (Amplify Gen 2 cross-repo schema compatibility — pmcp-run owns the table, agent reads via env var)

**Key decisions to lock in before writing code:**
- pmcp-run owns the canonical AgentRegistry DynamoDB table; Step Functions migrates to it on a cutover date (not ongoing sync)
- New Amplify models (`AgentConfig`, `AgentExecution`) have zero Amplify relationship links to `McpServer`, `Deployment`, or other existing models
- Feature flag the Agents tab: deploy backend changes first, validate they do not affect existing MCP server hosting, then enable UI
- Agent Lambda reads table name from `AGENT_REGISTRY_TABLE` environment variable, not an Amplify-generated constant

### Phase 4: PMCP SDK Example

**Rationale:** Depends on a stable agent binary from all prior phases. The MCP Tasks spec was experimental as of late 2025 (per official roadmap); tracking an unstable spec is wasted work. Lowest risk to defer until the core platform is stable and the spec has settled.

**Delivers:** Durable agent exposed as MCP server with `TaskSupport::Required`, task state mapping (running->working, completed->completed, failed->failed, waiting_for_callback->input_required), progress reporting via MCP notifications, cancellation support, reference example in `rust-mcp-sdk/examples/`

**Addresses features:** Durable agent as MCP server, task status mapping, progress reporting via notifications, cancellation, multi-tool task server (multiple agent configs as separate tools)

**Avoids:** MCP Tasks spec instability — use types from `pmcp-tasks` crate which tracks the spec; do not hand-roll task protocol types

**Key decisions to lock in before writing code:**
- State mapping table defined upfront (not discovered during implementation)
- Task metadata stored in DynamoDB using `pmcp-tasks` `DynamoDbTaskStore` if available; do not checkpoint task state inside the durable execution

### Phase Ordering Rationale

- **Channels first** because the webhook-vs-WebSocket constraint cascades; getting it wrong means an architectural rewrite, not a small fix. Security model is easiest to get right from the start.
- **Agent Teams second** because `ctx.invoke()` works without channels; can start after channel architecture is locked even before channel adapters are all implemented. Delegation loop prevention must be in the initial design.
- **Agents Tab third** because it depends on the stable schema from Phases 1-2; DynamoDB schema migrations are risky while other schema-changing phases are in flight simultaneously.
- **PMCP SDK Example last** because it documents the completed platform; the experimental MCP Tasks spec warrants waiting for stability before committing implementation effort.

### Research Flags

Phases needing deeper research during planning:
- **Phase 1 (Discord adapter):** Discord Ed25519 webhook signature verification specifics; interaction deferred response (type 5) flow. Only needed if Discord is confirmed in scope for Phase 1 (vs. deferred).
- **Phase 1 (Slack approval UX):** Slack Block Kit interactive button format for approval messages with approve/deny/modify actions. Verify current Block Kit schema against Slack docs before implementing.
- **Phase 2 (pmcp DynamicServerManager):** Verify `add_tool()` / `remove_tool()` API surface against pmcp 2.0.2 source before implementing synthetic tool generation. The team orchestrator uses this API but the exact method signatures need confirmation.
- **Phase 4 (MCP Tasks spec):** Re-check `pmcp-tasks` crate API and MCP Tasks spec version before starting Phase 4. The spec was experimental in late 2025 and may have changed.

Phases with standard patterns (skip or reduce research-phase):
- **Phase 1 (Webhook Receiver Lambda):** Pattern is proven and documented in research; `lambda_http` + HMAC-SHA256 is a standard, low-surprise pattern.
- **Phase 3 (Agents Tab CRUD):** UI pattern exists in Step Functions Agent project (12+ pages to port); LCARS components established in pmcp-run. Straightforward port, not novel architecture.
- **Phase 3 (EventBridge Scheduler):** SAM YAML declarative scheduling is documented and standard; only programmatic schedule management (create/update/delete from UI) needs `aws-sdk-scheduler`.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | All crate versions verified against crates.io (2026-03-24); agent binary needs zero new deps; webhook receiver needs only `lambda_http` + `hmac` |
| Features | HIGH | Direct codebase analysis of zeroclaw (channels, 12 implementations), Step Functions Agent UI (14 pages to port), pmcp-run (LCARS, Amplify Gen 2 schema), pmcp-tasks (MCP Tasks) |
| Architecture | HIGH for SDK integration points (verified from source); MEDIUM for cross-system data flow (inferred from multi-codebase reads); LOW for MCP Tasks (spec experimental) | Channel Router verified against zeroclaw; `ctx.invoke()` for teams verified in SDK internals; pmcp-run schema extension inferred |
| Pitfalls | HIGH | 12 pitfalls with specific consequences and prevention strategies; several verified from direct source analysis (checkpoint limits from SDK, DynamoDB schema from three repos) |

**Overall confidence:** HIGH for core architecture and stack; MEDIUM for pmcp-run cross-repo integration details; LOW for MCP Tasks (experimental spec, verify before Phase 4)

### Gaps to Address

- **Discord integration scope in Phase 1:** Research covers Discord REST + webhook pattern, but Discord is listed as a differentiator (not table stakes). Confirm whether Discord adapter is in Phase 1 or deferred to v3+ before planning Phase 1 in detail.

- **AgentRegistry table ownership migration plan:** Three systems currently hold conceptual ownership. pmcp-run is the recommended canonical owner, but the migration from Step Functions CDK tables has not been designed. Needs a concrete migration script and cutover date before Phase 3 planning begins.

- **Checkpoint baseline for Agent Teams:** v1.0 analysis showed ~225KB for a 10-iteration agent loop. With channels adding ~15KB (approval events), baseline is ~240KB. A concrete per-specialist response size limit (recommended: 10KB) and orchestrator max_iterations cap (recommended: 5-8) need to be confirmed before Phase 2 implementation.

- **`pmcp-tasks` crate API stability:** The pmcp-tasks crate exists with InMemoryTaskStore, DynamoDbTaskStore, and TaskRouter. MCP Tasks spec was marked experimental in late 2025. Re-verify crate API against current pmcp-tasks source before starting Phase 4.

- **EventBridge Scheduler IAM permissions:** `aws-sdk-scheduler` requires `scheduler:CreateSchedule`, `scheduler:UpdateSchedule`, `scheduler:DeleteSchedule`, and IAM PassRole for the Lambda target execution role. Confirm these are added to the SAM template before Phase 3 deployment.

## Sources

### Primary (HIGH confidence)

- `/Users/guy/Development/mcp/lambda-durable-execution-rust/examples/Cargo.toml` + `Cargo.lock` — dependency versions verified
- `/Users/guy/projects/LocalAgent/zeroclaw/src/channels/` — Channel trait, 12 implementations, deny-by-default security model (direct source analysis)
- `/Users/guy/projects/step-functions-agent/ui_amplify/src/pages/` — 14 existing UI pages (source for Phase 3 porting patterns)
- `/Users/guy/Development/mcp/sdk/pmcp-run/` — LCARS design system, Amplify Gen 2 schema, authenticated routes
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/server/dynamic.rs` — pmcp `DynamicServerManager` API (verified from source)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/crates/pmcp-tasks/` — MCP Tasks: InMemoryTaskStore, DynamoDbTaskStore, TaskRouter
- `/Users/guy/Development/mcp/lambda-durable-execution-rust/src/` — SDK internals: checkpoint limits, `wait_for_callback`, `ctx.invoke()`, `run_in_child_context` fallback behavior
- [crates.io: aws-sdk-scheduler](https://crates.io/crates/aws-sdk-scheduler) — v1.97.0, verified 2026-03-24
- [crates.io: lambda_http](https://crates.io/crates/lambda_http) — v1.1.2, verified 2026-03-24
- [crates.io: hmac](https://crates.io/crates/hmac) — v0.12.1, verified 2026-03-24
- [crates.io: reqwest](https://crates.io/crates/reqwest) — v0.13.2, verified 2026-03-24

### Secondary (MEDIUM confidence)

- [MCP Tasks Specification (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks) — task lifecycle, status states, polling, cancellation
- [MCP 2026 Roadmap](http://blog.modelcontextprotocol.io/posts/2026-mcp-roadmap/) — Tasks experimental status
- [Multi-agent orchestration patterns 2025-2026](https://www.chanl.ai/blog/multi-agent-orchestration-patterns-production-2026) — hierarchical pattern, model tiering 40-60% cost data
- [Human-in-the-loop for AI agents](https://www.permit.io/blog/human-in-the-loop-for-ai-agents-best-practices-frameworks-use-cases-and-demo) — approval workflows, channel-based notifications
- [EventBridge Scheduler docs](https://docs.aws.amazon.com/scheduler/latest/UserGuide/getting-started.html) — Lambda target scheduling, one-time vs recurring

### Tertiary (LOW confidence)

- MCP Tasks spec stability — experimental as of late 2025; spec may evolve before Phase 4 implementation. Re-verify before building.

---
*Research completed: 2026-03-24*
*Ready for roadmap: yes*
