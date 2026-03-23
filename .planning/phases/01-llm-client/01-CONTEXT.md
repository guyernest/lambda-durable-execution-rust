# Phase 1: LLM Client - Context

**Gathered:** 2026-03-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Extract and adapt the UnifiedLLMService from the existing call_llm_rust Lambda into this repo as a self-contained module within the agent example binary. Provides typed LLM request/response with Anthropic and OpenAI transformers, Secrets Manager auth, and error classification for retry.

</domain>

<decisions>
## Implementation Decisions

### Code extraction strategy
- **D-01:** Copy and adapt — copy the relevant source files (models, service, transformers, secrets, error) from `~/projects/step-functions-agent/lambda/call_llm_rust/src/` into this repo. Adapt for the durable agent context (remove Lambda handler, telemetry/OpenTelemetry). Clean break from the original, no shared dependency.
- **D-02:** Code lives under `examples/src/bin/mcp_agent/` as part of the agent binary. Self-contained, no crate boundary. The module structure mirrors the original: `models.rs`, `service.rs`, `transformers/`, `secrets.rs`, `error.rs`.

### Provider scope
- **D-03:** Include Anthropic and OpenAI transformers only. Gemini and Bedrock transformers are NOT copied for PoC — the TransformerRegistry pattern makes them trivial to add later.
- **D-04:** Keep the same `MessageTransformer` trait and `TransformerRegistry` pattern from the original code. Register `anthropic_v1` and `openai_v1` transformers.

### Secrets & auth pattern
- **D-05:** LLM service initialization (including Secrets Manager fetch) happens per Lambda invocation, OUTSIDE durable steps. Secrets are NOT checkpointed — they should never appear in checkpoint data.
- **D-06:** Keep the existing ProviderConfig auth model as-is (auth_header_name, auth_header_prefix, secret_path, secret_key_name). Drop-in compatible with AgentRegistry — no schema changes needed for LLM auth.

### Performance philosophy
- **D-07:** No micro-optimization needed within the agent. Agents are inherently slow (remote LLM calls + reasoning). The performance win comes from eliminating Step Functions overhead, not from optimizing internal code paths. For speed-critical operations, direct MCP server calls bypass the agent entirely.

### Claude's Discretion
- Module structure within `examples/src/bin/mcp_agent/`
- Error type design (adapt ServiceError for durable agent needs)
- Test strategy (unit tests for transformers, integration patterns)
- Type adaptations for checkpoint serialization (ensure LLMResponse is Serialize/Deserialize for `ctx.step()` caching)
- Whether to use `async-trait` or native async traits (Rust edition 2021 with MSRV 1.88 supports async fn in traits)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing LLM Caller (source for extraction)
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/models.rs` — LLMInvocation, LLMResponse, ProviderConfig, UnifiedMessage, ContentBlock, UnifiedTool types
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/service.rs` — UnifiedLLMService with process() method, secret retrieval, transformer dispatch
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/transformers/mod.rs` — MessageTransformer trait, TransformerRegistry
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/transformers/anthropic.rs` — AnthropicTransformer (tool_use/tool_result content blocks)
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/transformers/openai.rs` — OpenAITransformer (function calling format)
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/secrets.rs` — SecretManager with get_api_key()
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/error.rs` — ServiceError types
- `~/projects/step-functions-agent/lambda/call_llm_rust/Cargo.toml` — Dependencies to bring over

### Durable SDK integration points
- `src/runtime/mod.rs` — Handler wrapper pattern (with_durable_execution_service)
- `examples/src/bin/hello_world/main.rs` — Example handler structure to follow
- `src/context/durable_context/step.rs` — How step() works (result must be Serialize + DeserializeOwned)

### Research
- `.planning/research/STACK.md` — Technology recommendations, reqwest with rustls-tls, hand-rolled Anthropic client rationale
- `.planning/research/ARCHITECTURE.md` — Component boundaries, where LLM client fits in the agent architecture

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `UnifiedLLMService` (call_llm_rust): Full working multi-provider LLM service with transformer pattern — copy models.rs, service.rs, transformers/, secrets.rs, error.rs
- `MessageTransformer` trait: Provider abstraction with transform_request/transform_response — battle-tested with 4 providers
- `SecretManager`: AWS Secrets Manager client with caching — copy secrets.rs directly
- Example binary structure (`examples/src/bin/*/main.rs`): Established pattern for adding new example binaries with SAM integration

### Established Patterns
- `Serialize + Deserialize` on all types: SDK's `ctx.step()` requires serde traits — existing models.rs types already derive these
- `reqwest::Client` with builder: Timeout, connection pooling, custom headers — existing service.rs has this
- `tracing` for logging: Already used by both the SDK and the existing LLM caller

### Integration Points
- The agent binary will depend on `lambda-durable-execution-rust` as a path dependency (like other examples)
- LLM response (`LLMResponse`) will be the return type of `ctx.step("llm-call", ...)` — must be checkpoint-serializable
- `ProviderConfig` will come from AgentRegistry in Phase 2 — design types to be compatible

</code_context>

<specifics>
## Specific Ideas

- "If we can build it as a drop-in replacement to the step functions, it will be best" — the ProviderConfig schema match is key, same auth model, same transformer IDs
- Agents are for depth, not speed — no micro-optimization, the Step Functions overhead elimination is the performance win
- Direct MCP server calls (without agent) remain the fast path for speed-critical operations

</specifics>

<deferred>
## Deferred Ideas

- Gemini and Bedrock transformers — copy from call_llm_rust when needed (v2)
- OpenTelemetry metrics for token usage and latency — Phase 4 observability
- Streaming LLM responses — explicitly out of scope per PROJECT.md

</deferred>

---

*Phase: 01-llm-client*
*Context gathered: 2026-03-23*
