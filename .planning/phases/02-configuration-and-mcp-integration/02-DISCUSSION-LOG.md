# Phase 2: Configuration and MCP Integration - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-23
**Phase:** 02-configuration-and-mcp-integration
**Areas discussed:** AgentRegistry schema, MCP server connectivity, Tool name collisions

---

## AgentRegistry Schema

### mcp_servers field structure

| Option | Description | Selected |
|--------|-------------|----------|
| URL array (simplest) | Simple list of endpoints: ["https://...", "https://..."] | ✓ |
| Named objects with auth | [{name, url, auth: {type, secret_path}}, ...] | |
| Match existing tools field | [{server_id, url, enabled}, ...] | |

**User's choice:** URL array (simplest)

### Provider config source

| Option | Description | Selected |
|--------|-------------|----------|
| Read existing fields | Read llm_provider + llm_model, map to ProviderConfig in code | ✓ |
| Store full ProviderConfig | Add provider_config JSON object to AgentRegistry | |
| You decide | Claude picks | |

**User's choice:** Read existing fields
**Notes:** Must preserve operator workflow — different steps update different records (providers, API keys, models, agents). The join happens in agent code, not DynamoDB.

---

## MCP Server Connectivity

### Deployment model

| Option | Description | Selected |
|--------|-------------|----------|
| Lambda + Web Adapter | Lambda behind API Gateway/Function URL with Web Adapter | ✓ |
| Long-running containers | ECS/Fargate with persistent endpoints | |
| Mixed | Some Lambda, some containers | |

**User's choice:** Lambda + Web Adapter

### Authentication

| Option | Description | Selected |
|--------|-------------|----------|
| No auth (VPC/IAM) | Same VPC or IAM auth at API Gateway level | ✓ |
| Bearer tokens | Per-server tokens from Secrets Manager | |
| IAM Sig v4 | API Gateway with IAM authorization | |

**User's choice:** IAM/VPC for PoC
**Notes:** OAuth deferred — will use PMCP SDK capabilities. Two patterns: user-forwarded tokens from interface, or M2M tokens.

### Scale

| Option | Description | Selected |
|--------|-------------|----------|
| 1-3 servers | Sequential connection fine | ✓ |
| 3-10 servers | Parallel connection helps | |
| Varies widely | Build for parallel | |

**User's choice:** 1-3 servers

---

## Tool Name Collisions

### Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Fail fast | Error if duplicate names | |
| Prefix with server | Auto-prefix: server__tool | ✓ |
| First wins | First server takes precedence | |

**User's choice:** Prefix with server

### Prefix format

| Option | Description | Selected |
|--------|-------------|----------|
| Index-based (s0__, s1__) | Numeric, short, not self-documenting | |
| Host-based (calc__, wiki__) | Extract from URL hostname, readable | ✓ |
| Always prefix | Consistent naming regardless of collisions | |

**User's choice:** Host-based, always prefix all tools

---

## Claude's Discretion

- DynamoDB client setup
- MCP schema-to-Claude translation
- Error types for config/MCP failures
- Host prefix extraction from URLs
- Test strategy

## Deferred Ideas

- OAuth MCP server auth — future, using PMCP SDK
- Parallel MCP connection — not needed for 1-3 servers
- MCP server health checks — future hardening
- Dynamic provider config table — maps in code for now
