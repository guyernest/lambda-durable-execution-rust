# Phase 3: Agent Loop - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-23
**Phase:** 03-agent-loop
**Areas discussed:** Handler input/output, MCP tool execution

---

## Handler Input/Output

### Input payload

| Option | Description | Selected |
|--------|-------------|----------|
| Agent name + messages | { agent_name, version, messages } — loads own config | ✓ |
| Full config + messages | { agent_config, messages } — no DynamoDB lookup | |
| Match Step Functions | Same format as existing SF agent | |

**User's choice:** Agent name + messages

### Output format

| Option | Description | Selected |
|--------|-------------|----------|
| Final message + metadata | Last assistant message + run stats | |
| Full conversation | Complete message history | |
| Match Step Functions | Same output as current SF agent for compatibility | ✓ |

**User's choice:** Match Step Functions output format

---

## MCP Tool Execution

### Connection strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Connect per tool call | Establish/teardown per call inside ctx.map() | |
| Cache clients per invocation | Connect once at start, reuse across iterations | ✓ |
| You decide | Claude picks | |

**User's choice:** Cache clients per invocation (behave like standard MCP client)

**Extended discussion:** User raised important points:
- Most tool calls are single (sequential reasoning), not parallel
- MCP servers are fast (Rust, same VPC) but some calls can be long (database queries, remote agents, MCP tasks)
- Small number of MCP servers per agent (subset of tools per task)
- Question about durable Lambda idle model for long-running calls

**Clarification provided:** Lambda does NOT suspend during ctx.step() execution. It only suspends for explicit ctx.wait()/ctx.wait_for_callback()/retry backoff. Long MCP tool calls keep the Lambda active. TCP connections survive across iterations. On replay (after suspension), connections are re-established at handler start.

---

## Claude's Discretion

- Handler function structure and wiring
- Step Functions output format extraction
- Error types for agent-level failures
- Test strategy
- Message assembly pattern
- Max iterations check placement

## Deferred Ideas

- ctx.wait_for_callback() for long-running tool calls — v2
- Streaming — out of scope
- Agent-to-agent delegation — future
- Context window management — Phase 4+
