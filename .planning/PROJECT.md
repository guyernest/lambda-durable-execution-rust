# Durable Lambda MCP Agent Platform

## What This Is

A unified platform for AI agents and MCP servers, built on Durable Lambda execution. Extends pmcp.run from an MCP server hosting service into a full agent platform — agents are config-driven Durable Lambdas that discover and use MCP tools, communicate via multi-channel side-channels (Slack, Discord, local agents), and can orchestrate as teams. Replaces Step Functions orchestration entirely.

## Core Value

A single Durable Lambda replaces the entire Step Functions orchestration — the agent loop is plain Rust code with checkpointed LLM calls and MCP tool executions, extended with channels for human/agent interaction and team coordination, all managed through pmcp.run.

## Current Milestone: v2.0 Integration Plan

**Goal:** Extend pmcp.run into a unified platform for MCP servers AND AI agents, replacing Step Functions with Durable Lambda, adding multi-channel communication, and enabling agent teams.

**Target features:**
- Single config-driven Durable Agent Lambda deployable via pmcp-run
- Channels abstraction (Slack, Discord, WhatsApp, local agent) for approval, interaction, and inter-agent communication
- PMCP SDK reference example showcasing MCP Tasks and client patterns
- Agent Teams with dynamic MCP server generation exposing agents as tools
- pmcp-run Agents tab for agent config, execution, scheduling, and history
- Migration of Step Functions Agent management features into pmcp-run

## Requirements

### Validated

- ✓ Multi-provider LLM client with typed request/response, error classification, and Secrets Manager auth — Phase 1
- ✓ AgentRegistry config loading from DynamoDB with provider mapping and MCP server tool discovery with prefix routing — Phase 2
- ✓ Complete durable agent loop — LLM call, parallel MCP tool execution, incremental message history, child context isolation, max iterations guard — Phase 3
- ✓ Observability — token tracking, iteration metadata in AgentResponse, per-step structured logging — Phase 4
- ✓ Deployment — SAM template with DurableConfig, AgentRegistry DynamoDB table, validation script — Phase 5

### Active

- [ ] Single generic Durable Agent Lambda reading all configuration from registry
- [ ] Channels abstraction generalizing wait_for_callback into named communication channels
- [ ] Agent Teams with dynamic MCP server generation and orchestrated execution
- ✓ pmcp-run Agents tab for agent lifecycle management — Phase 7
- ✓ PMCP SDK reference example for MCP client patterns — Phase 6
- [ ] Migration of Step Functions Agent management capabilities

### Out of Scope

- Gemini and Bedrock/Nova transformers — Anthropic and OpenAI sufficient for platform integration
- Building new MCP servers — this milestone integrates existing pmcp-run hosted servers
- Replacing pmcp-run's existing MCP hosting features — additive only
- Mobile clients — web-first via pmcp-run LCARS UI

## Context

### Existing Assets (v1.0 Delivered)

- **Durable Execution SDK** (this repo): `step()`, `map()`, `parallel()`, `wait()`, `wait_for_callback()` with checkpointing and replay. 332 tests, production-quality.
- **Durable MCP Agent** (this repo, `examples/src/bin/mcp_agent/`): Working agent binary with LLM client, MCP integration, agent loop, observability, SAM deployment. All 30 v1 requirements complete.

### Integration Targets

- **Step Functions Agent** (`~/projects/step-functions-agent/`): Rich Amplify Gen 2 management UI (12+ pages), AgentRegistry, ToolRegistry, MCPServerRegistry, LLM Models Registry, execution history, metrics, cost tracking, approval dashboard. CDK Python deployment.
- **pmcp-run** (`~/Development/mcp/sdk/pmcp-run/`): MCP server hosting platform with Next.js LCARS UI, Amplify Gen 2 backend, multi-tenant SaaS, deployment pipeline (OpenAPI/GraphQL/SQL schema-to-server), registry, monitoring.
- **PMCP SDK** (`~/Development/mcp/sdk/rust-mcp-sdk/`): Rust MCP SDK (`pmcp` crate) used to build MCP servers deployed to pmcp-run. Client and server support, HTTP/SSE transport.
- **ZeroClaw** (`~/projects/LocalAgent/zeroclaw/`): Rust agent runtime with 12+ channel transports (Telegram, Discord, Slack, WhatsApp, Activity, webhook, CLI), trait-driven channel abstraction, deny-by-default security, supervised listeners with auto-restart. Reference architecture for the channels system.

### Architecture Decision

The Durable Lambda agent replaces Step Functions entirely. The agent loop is Rust code — no state machine definition, no JSONata, no explicit tool routing. MCP is native (the agent IS an MCP client). Channels generalize `wait_for_callback()` for human approval, local agent tasks, and inter-agent communication. Agent Teams use dynamic MCP servers where each agent is exposed as a tool.

### Runtime Constraint

AWS Durable Execution requires `nodejs24.x` runtime with `AWS_LAMBDA_EXEC_WRAPPER=/var/task/bootstrap` for Rust binaries.

## Constraints

- **Tech stack**: Rust (edition 2021, MSRV 1.88), AWS Lambda with Durable Execution
- **Dependencies**: `lambda-durable-execution-rust` (this crate), `pmcp` (MCP SDK)
- **MCP transport**: HTTP/SSE (stdio not viable in Lambda)
- **Checkpoint limits**: 750KB per checkpoint batch
- **pmcp-run compatibility**: Additive features to existing platform, no breaking changes
- **Cross-team delivery**: Each phase should be independently assignable to different teams
- **Channel security**: Deny-by-default model (following zeroclaw patterns)

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Standardize on MCP for all tool interactions | Eliminates DynamoDB Tool Registry, simplifies discovery | ✓ Good (v1.0) |
| Extract UnifiedLLMService from call_llm_rust | Reuse proven multi-provider abstraction | ✓ Good (v1.0) |
| Build agent as example binary in this repo | Keeps PoC close to SDK; extract to separate crate later | ✓ Good (v1.0) |
| Extend AgentRegistry with mcp_servers field | Additive change, existing agents unaffected | ✓ Good (v1.0) |
| Single generic agent binary, not per-agent | Config-driven from registry; custom loops are rare exception | — Pending |
| pmcp-run as unified platform | Already has hosting pipeline, registry, multi-tenant UI, deployment | — Pending |
| Channels model from zeroclaw | Proven trait abstraction with 12+ transports, security model | — Pending |
| Agent Teams via dynamic MCP server | Agents as tools, orchestrator pattern, maps to ctx.parallel/map | — Pending |

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
*Last updated: 2026-03-25 after Phase 7 completion*
