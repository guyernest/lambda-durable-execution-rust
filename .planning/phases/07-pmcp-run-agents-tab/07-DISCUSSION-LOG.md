# Phase 7: pmcp-run Agents Tab - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 07-pmcp-run-agents-tab
**Areas discussed:** Agent Lambda deployment, Data model + API design, Execution + status tracking, Metrics + cost tracking

---

## Agent Lambda Deployment

| Option | Description | Selected |
|--------|-------------|----------|
| CDK escape hatch in Amplify | Add Rust Lambda as CDK construct in amplify/backend.ts | ✓ |
| Separate SAM stack | Agent Lambda deployed independently via SAM | |
| Shared binary, per-org config | One generic Lambda, behavior from invocation payload | |

**User's choice:** CDK escape hatch in Amplify

### Follow-up: Isolation model

| Option | Description | Selected |
|--------|-------------|----------|
| One shared Lambda | Single Lambda per environment, config in payload | ✓ |
| Per-org Lambda | Each organization gets its own isolated Lambda | |
| You decide | Claude picks based on patterns | |

**User's choice:** One shared Lambda
**Notes:** pmcp-run is designed as private per-org deployment, not multi-tenant SaaS. Lambda tenant separation available if needed later.

---

## Data Model + API Design

| Option | Description | Selected |
|--------|-------------|----------|
| Fresh Amplify model | New AgentConfig in resource.ts, no Step Functions compatibility | ✓ |
| Compatible with AgentRegistry | Match existing DynamoDB schema fields | |
| You decide | Claude picks | |

**User's choice:** Fresh Amplify model

### Follow-up: MCP server selection

| Option | Description | Selected |
|--------|-------------|----------|
| From pmcp-run registry | Multi-select from deployed servers only | ✓ |
| Both registry + custom URLs | Registry default + arbitrary external URLs | |
| You decide | Claude picks | |

**User's choice:** From pmcp-run registry only — ensures agents use trusted servers

---

## Execution + Status Tracking

| Option | Description | Selected |
|--------|-------------|----------|
| Agent writes to DynamoDB | AgentExecution table, UI polls via GraphQL | |
| Poll durable execution API | Use GetDurableExecution for status | |
| Both DynamoDB + durable API | DynamoDB for fast queries, durable API for details | ✓ |

**User's choice:** Both — DynamoDB for progress tracking, status, and final response. AgentExecution holds the execution ID for querying the durable API for full message history when needed.

---

## Metrics + Cost Tracking

| Option | Description | Selected |
|--------|-------------|----------|
| Agent response metadata | Token counts in AgentExecution DynamoDB | |
| CloudWatch custom metrics | Direct PutMetricData calls | |
| Both DynamoDB + CloudWatch | Per-execution in DynamoDB, trends in CloudWatch | ✓ |

**User's choice:** Both — aggregate token counts in Lambda, store in DynamoDB as summary. Time-series metrics via CloudWatch EMF format (embedded in logs, not direct PutMetricData which is too expensive). Not real-time.

---

## Claude's Discretion

- LCARS component selection
- GraphQL naming conventions
- Pagination strategy
- Form field ordering
- Chart library choice
- Agent Lambda progress reporting frequency

## Deferred Ideas

- Scheduled/triggered execution (EventBridge) — v3
- Approval dashboard — Phase 8
- Live streaming — v3
- Version management — v3
- Test prompt library — v3
