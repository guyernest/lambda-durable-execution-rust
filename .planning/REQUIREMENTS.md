# Requirements: Durable Lambda MCP Agent Platform

**Defined:** 2026-03-24
**Core Value:** A single Durable Lambda replaces Step Functions orchestration -- extended with channels for human/agent interaction, teams for multi-agent coordination, and pmcp.run for unified management.

## v2 Requirements

Requirements for v2.0 Integration Plan milestone. Each maps to roadmap phases.

### Channels Abstraction

- [ ] **CHAN-01**: Channel trait with deliver() and health_check() methods, adapted for Lambda (no listen())
- [ ] **CHAN-02**: Channel registry mapping named channels to adapters, configured per-agent in AgentRegistry
- [ ] **CHAN-03**: Durable channel send via ctx.step() for idempotent delivery on replay
- [ ] **CHAN-04**: Durable channel receive via wait_for_callback() with callback ID as channel correlation ID
- [ ] **CHAN-05**: Slack channel adapter using Web API chat.postMessage for outbound delivery
- [ ] **CHAN-06**: Webhook channel adapter for generic HTTP callback send/receive
- [ ] **CHAN-07**: Discord channel adapter using REST API for outbound delivery and webhook interactions for inbound
- [ ] **CHAN-08**: Local agent channel bridging cloud agents to zeroclaw instances via webhook/SSE transport
- [ ] **CHAN-09**: Webhook Receiver Lambda binary that validates signatures, acknowledges within 3 seconds, and calls SendDurableExecutionCallbackSuccess
- [ ] **CHAN-10**: Deny-by-default security model — channels explicitly allowed per-agent, no implicit access
- [ ] **CHAN-11**: Channel configuration schema as additive field in AgentRegistry DynamoDB table

### Approval Flow

- [ ] **APPR-01**: Tool danger classification in agent config with allowlist/denylist patterns (e.g., "deploy__*", "db__delete")
- [ ] **APPR-02**: Approval gate in agent loop — agent pauses, sends approval request via configured channel, waits for response
- [ ] **APPR-03**: Approval timeout with default-deny using CallbackConfig::with_timeout()
- [ ] **APPR-04**: Approval response supports modification of tool arguments before execution

### Agent Teams

- [ ] **TEAM-01**: Agent-as-MCP-tool wrapping — each team member exposed as a callable tool to the orchestrator
- [ ] **TEAM-02**: Dynamic tool definition generation from team member agent configs at discovery time
- [ ] **TEAM-03**: Sequential team execution via ctx.step() for pipeline patterns
- [ ] **TEAM-04**: Parallel team execution via ctx.map() for fan-out patterns
- [ ] **TEAM-05**: Delegation depth guard with configurable maximum (3-5) passed through AgentRequest
- [ ] **TEAM-06**: Visited-agent tracking to prevent circular delegation loops
- [ ] **TEAM-07**: Shared context via S3 Files as MCP resources accessible to all team members
- [ ] **TEAM-08**: Team configuration schema in AgentRegistry (member agents, orchestrator settings)
- [ ] **TEAM-09**: Specialist response summarization before checkpointing to stay within 750KB batch limit

### pmcp-run Agents Tab

- [x] **PMCP-01**: Agent list view in LCARS design system showing name, status, model, MCP servers
- [x] **PMCP-02**: Agent create/edit form with instructions, model selection, MCP server selection, channel config, parameters
- [x] **PMCP-03**: Agent delete with confirmation
- [x] **PMCP-04**: Model registry with provider, pricing, and capabilities (migrated from Step Functions LLMModels table)
- [x] **PMCP-05**: MCP server selector populated from pmcp-run's existing server registry
- [x] **PMCP-06**: API key management for LLM providers via Secrets Manager
- [x] **PMCP-07**: On-demand agent execution from UI with input textarea and agent selector
- [x] **PMCP-08**: Execution status tracking with running/completed/failed badges
- [x] **PMCP-09**: Execution history list with pagination, status filter, and agent filter
- [x] **PMCP-10**: Execution detail view with full conversation history rendering
- [ ] **PMCP-11**: Metrics dashboard with token usage charts and execution success rates
- [ ] **PMCP-12**: Cost tracking by model and agent with trend visualization

### PMCP SDK Example

- [x] **SDK-01**: Reference MCP agent example in rust-mcp-sdk demonstrating LLM + MCP tool loop with Durable Lambda checkpointing
- [x] **SDK-02**: Client-side MCP Tasks handling — agent detects task responses from long-running tools and polls via ctx.wait_for_condition() until completion
- [x] **SDK-03**: Structured progress logging with iteration count, tokens used, and tools called per agent loop iteration

## v1 Requirements (Complete)

All 30 v1 requirements delivered across 5 phases. See v1 REQUIREMENTS.md for details.

### LLM Client — Complete (Phase 1)
- [x] **LLM-01** through **LLM-07**: UnifiedLLMService, Anthropic/OpenAI transformers, provider config, Secrets Manager auth, error classification, function_calls extraction

### Configuration — Complete (Phase 2)
- [x] **CONF-01** through **CONF-04**: AgentRegistry loading, system_prompt/model/params, mcp_servers array, durable ctx.step() caching

### MCP Integration — Complete (Phases 2-3)
- [x] **MCP-01** through **MCP-06**: Server connection, tool discovery, schema translation, tool execution, error handling, fail-fast

### Agent Loop — Complete (Phase 3)
- [x] **LOOP-01** through **LOOP-07**: Agentic loop, durable steps with retry, parallel map, child context isolation, message history, max iterations, final response

### Observability — Complete (Phase 4)
- [x] **OBS-01** through **OBS-03**: Token tracking, iteration metadata, structured logging

### Deployment — Complete (Phase 5)
- [x] **DEPL-01** through **DEPL-03**: SAM template, IAM permissions, end-to-end validation

## v3 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Additional Channels
- **CHAN-12**: WhatsApp channel adapter via WhatsApp Business API
- **CHAN-13**: Signal channel adapter (requires Signal Bridge — no public bot API)
- **CHAN-14**: Email channel adapter via SES

### Advanced Agent Teams
- **TEAM-10**: Agent handoff — explicit transfer of control between agents (OpenAI Agents SDK pattern)
- **TEAM-11**: Team-level cost budget enforcement across all members
- **TEAM-12**: Consensus/voting pattern — multiple agents analyze independently, orchestrator aggregates

### pmcp-run Advanced
- **PMCP-13**: Scheduled agent execution via EventBridge Scheduler with cron/rate-based triggers from UI
- **PMCP-14**: Triggered agent execution from events (S3, DynamoDB streams, SNS)
- **PMCP-15**: Approval dashboard — centralized pending approvals view with approve/deny/modify from UI
- **PMCP-16**: Test prompt library — save and reuse test prompts per agent
- **PMCP-17**: Agent version management with draft/active/archived promotion
- **PMCP-18**: Live execution streaming via WebSocket or SSE

### PMCP SDK Advanced
- **SDK-04**: Cancellation support — client sends tasks/cancel, durable execution terminates gracefully
- **SDK-05**: Input-required flow mapping wait_for_callback to MCP Tasks input_required status
- **SDK-06**: Multi-tool task server — single MCP server exposing multiple agent configs as separate tools

### Migration
- **MIG-01**: Model costs migration from Step Functions LLMModelsRegistry to pmcp-run
- **MIG-02**: Execution history migration from Step Functions to new DynamoDB format
- **MIG-03**: MCP server registry reconciliation between Step Functions and pmcp-run registries

## Out of Scope

| Feature | Reason |
|---------|--------|
| Slack Socket Mode / persistent WebSocket in Lambda | Lambda cannot maintain persistent connections; use webhook-based integration |
| Channel listeners running inside Lambda | Lambda is invocation-based; use wait_for_callback() with external triggers |
| Custom orchestration DSL for agent teams | Recreates Step Functions; the LLM IS the orchestrator via plain Rust code |
| Real-time bidirectional streaming in Lambda | Durable Lambda is request-response with checkpointing; use polling via MCP Tasks |
| Per-message conversation persistence in channels | Each invocation is stateless; use MCP memory tool if cross-invocation memory needed |
| Full zeroclaw runtime port | Port the Channel trait + security model only; listener supervision is local-agent concern |
| Agent-to-agent communication via shared DynamoDB | Use MCP resources (S3 Files) for shared state; DynamoDB creates coupling and race conditions |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| SDK-01 | Phase 6 | Complete |
| SDK-02 | Phase 6 | Complete |
| SDK-03 | Phase 6 | Complete |
| PMCP-01 | Phase 7 | Complete |
| PMCP-02 | Phase 7 | Complete |
| PMCP-03 | Phase 7 | Complete |
| PMCP-04 | Phase 7 | Complete |
| PMCP-05 | Phase 7 | Complete |
| PMCP-06 | Phase 7 | Complete |
| PMCP-07 | Phase 7 | Complete |
| PMCP-08 | Phase 7 | Complete |
| PMCP-09 | Phase 7 | Complete |
| PMCP-10 | Phase 7 | Complete |
| PMCP-11 | Phase 7 | Pending |
| PMCP-12 | Phase 7 | Pending |
| CHAN-01 | Phase 8 | Pending |
| CHAN-02 | Phase 8 | Pending |
| CHAN-03 | Phase 8 | Pending |
| CHAN-04 | Phase 8 | Pending |
| CHAN-05 | Phase 8 | Pending |
| CHAN-06 | Phase 8 | Pending |
| CHAN-07 | Phase 8 | Pending |
| CHAN-08 | Phase 8 | Pending |
| CHAN-09 | Phase 8 | Pending |
| CHAN-10 | Phase 8 | Pending |
| CHAN-11 | Phase 8 | Pending |
| APPR-01 | Phase 8 | Pending |
| APPR-02 | Phase 8 | Pending |
| APPR-03 | Phase 8 | Pending |
| APPR-04 | Phase 8 | Pending |
| TEAM-01 | Phase 9 | Pending |
| TEAM-02 | Phase 9 | Pending |
| TEAM-03 | Phase 9 | Pending |
| TEAM-04 | Phase 9 | Pending |
| TEAM-05 | Phase 9 | Pending |
| TEAM-06 | Phase 9 | Pending |
| TEAM-07 | Phase 9 | Pending |
| TEAM-08 | Phase 9 | Pending |
| TEAM-09 | Phase 9 | Pending |

**Coverage:**
- v2 requirements: 39 total
- Mapped to phases: 39/39
- Unmapped: 0

---
*Requirements defined: 2026-03-24*
*Last updated: 2026-03-24 after roadmap revision (Phases 6-9 reordered)*
