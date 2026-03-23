# Phase 1: LLM Client - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-23
**Phase:** 01-llm-client
**Areas discussed:** Code extraction strategy, Provider scope for PoC, Secrets & auth pattern

---

## Code Extraction Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Copy and adapt | Copy relevant source files into this repo, adapt for durable context. Clean break. | ✓ |
| Shared crate (git dep) | Extract core LLM logic into shared crate for both projects. More work upfront. | |
| Inline rewrite | Use existing code as reference, rewrite from scratch for durable agent needs. | |

**User's choice:** Copy and adapt
**Notes:** Gives speed and independence. Take what works, shed what doesn't.

### Code Placement

| Option | Description | Selected |
|--------|-------------|----------|
| Agent example module | Under examples/src/bin/mcp_agent/ — self-contained, no crate boundary | ✓ |
| SDK module (src/llm/) | New module in SDK's src/ directory — reusable but may bloat core SDK | |
| Separate workspace crate | New crate (crates/llm-client/) — clean separation, own Cargo.toml | |

**User's choice:** Agent example module
**Notes:** Simple, self-contained alongside the agent handler.

---

## Provider Scope for PoC

| Option | Description | Selected |
|--------|-------------|----------|
| All 4 (copy them all) | Minimal extra effort, full provider coverage from day one | |
| Anthropic + OpenAI only | Per requirements. Others trivial to add later via transformer pattern. | ✓ |
| Anthropic only (minimal) | Tightest PoC scope. Focus on proving durable loop with one provider. | |

**User's choice:** Anthropic + OpenAI only
**Notes:** TransformerRegistry pattern makes adding Gemini/Bedrock trivial later.

---

## Secrets & Auth Pattern

### API Key Retrieval

| Option | Description | Selected |
|--------|-------------|----------|
| Init per invocation (outside steps) | Fresh secrets each invocation, not checkpointed. Simple, matches existing pattern. | ✓ |
| Cached in durable step | Wrap init in ctx.step(). Saves Secrets Manager calls but serializes keys to checkpoints. | |
| Lazy per-provider | Fetch on first use per provider. Avoids fetching unused keys. | |

**User's choice:** Init per invocation (outside steps)
**Notes:** Secrets should never appear in checkpoint data. Secrets Manager calls are fast.

### Auth Model

| Option | Description | Selected |
|--------|-------------|----------|
| Keep as-is | Same ProviderConfig schema as call_llm_rust. Drop-in compatible with AgentRegistry. | ✓ |
| Simplify for PoC | Hardcode auth patterns per provider. Less flexible but simpler. | |
| You decide | Claude picks during implementation. | |

**User's choice:** Keep as-is
**Notes:** Drop-in compatibility with AgentRegistry, no schema changes needed.

---

## Additional Notes

User emphasized: "We don't need to over optimize for time. Agents are slow as they call remote LLM and apply reasoning. If speed is critical, call directly to MCP servers. The agents are called for depth or longer tasks. We save the Step Functions overhead, so the speedup is already there."

## Claude's Discretion

- Module structure within examples/src/bin/mcp_agent/
- Error type design
- Test strategy
- Type adaptations for checkpoint serialization
- async-trait vs native async traits

## Deferred Ideas

- Gemini and Bedrock transformers — v2
- OpenTelemetry metrics — Phase 4
- Streaming LLM responses — out of scope
