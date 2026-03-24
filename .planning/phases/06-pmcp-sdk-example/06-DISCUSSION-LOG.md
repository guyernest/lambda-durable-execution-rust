# Phase 6: PMCP SDK Example - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 06-pmcp-sdk-example
**Areas discussed:** Example packaging, Task granularity, Deployment model, Task variable schema

---

## Example Packaging

| Option | Description | Selected |
|--------|-------------|----------|
| PMCP SDK repo example | Lives in ~/Development/mcp/sdk/rust-mcp-sdk/examples/ as a reference for PMCP SDK users | ✓ |
| This repo's examples/ | Lives alongside mcp_agent in examples/src/bin/ | |
| Both — shared agent crate | Extract agent logic into a library crate, examples in both repos | |

**User's choice:** PMCP SDK repo example

### Follow-up: Dependency approach

| Option | Description | Selected |
|--------|-------------|----------|
| Git dependency on this repo | examples/Cargo.toml uses git = "..." | |
| Inline simplified agent | Write a simplified agent loop directly in the example | |
| You decide | Claude picks based on PMCP SDK example conventions | |

**User's choice:** Other — Clarified that this repo is a fork of the AWS Durable Lambda SDK. Official Rust SDK expected in weeks. Consider publishing fork to crates.io as interim step. Example should use simplified inline agent that works with either fork or official SDK.

---

## Task Granularity (reframed as Focus and Scope)

| Option | Description | Selected |
|--------|-------------|----------|
| One task per execution | Client submits one task, agent runs full loop | |
| Task per iteration | Each LLM call + tool execution is a separate task | |
| You decide | Claude picks best mapping | |

**User's choice:** Other — Clarified fundamental misunderstanding. MCP Tasks is about the CLIENT handling long-running server tools, not about wrapping the agent as a server. The example shows how MCP clients support tasks correctly — polling, sleeping, retrieving results.

### Follow-up: Client vs server focus

| Option | Description | Selected |
|--------|-------------|----------|
| Client-side only | Show how durable agent handles long-running MCP tools that return tasks | ✓ |
| Both client and server | Client handling AND wrap agent as MCP server | |
| Client + server demo pair | Demo MCP server with slow tool + agent as client | |

**User's choice:** Client-side only. Server-side Tasks deferred to Phase 9 (Agent Teams).

**Notes:** "The Agent example is not only for the MCP Tasks. It is an example of how to build MCP agents that can call MCP servers in an agent loop. The Tasks are only a small detail demonstrating the power of Durable Lambda."

---

## Deployment Model (reframed as Example Structure)

| Option | Description | Selected |
|--------|-------------|----------|
| Agent + demo server pair | Simple demo MCP server + agent client code | |
| Agent client code only | Just add task-aware client handling to agent | |
| You decide | Claude picks best approach | |

**User's choice:** Other — Clarified that the example is primarily about building MCP agents, not just about Tasks. PMCP SDK encourages stateless/serverless MCP patterns, and the agent is a natural extension.

---

## Task Handling Behavior (reframed from Task Variable Schema)

| Option | Description | Selected |
|--------|-------------|----------|
| ctx.wait() for poll sleep | Durable wait primitive to sleep between polls | |
| ctx.wait_for_condition() | SDK handles polling loop internally | ✓ |
| You decide | Claude picks best durable primitive | |

**User's choice:** wait_for_condition — "seems a simpler option, if the SDK can handle the polling loop internally"

---

## Claude's Discretion

- Simplified agent loop structure
- Demo MCP server (optional)
- Documentation depth and code comments
- Example configuration approach

## Deferred Ideas

- Server-side MCP Tasks — Phase 9 (Agent Teams)
- Publishing durable SDK fork to crates.io — separate effort
