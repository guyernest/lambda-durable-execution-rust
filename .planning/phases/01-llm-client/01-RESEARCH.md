# Phase 1: LLM Client - Research

**Researched:** 2026-03-23
**Domain:** Multi-provider LLM client extraction and adaptation (Anthropic + OpenAI)
**Confidence:** HIGH

## Summary

Phase 1 is an extraction and adaptation task, not a greenfield build. The existing `call_llm_rust` Lambda at `~/projects/step-functions-agent/lambda/call_llm_rust/src/` provides a battle-tested `UnifiedLLMService` with provider transformers (Anthropic, OpenAI, Gemini, Bedrock), Secrets Manager integration, and error handling. The task is to copy the relevant files (models, service, transformers/anthropic, transformers/openai, transformers/utils, secrets, error) into `examples/src/bin/mcp_agent/` and adapt them for the durable agent context.

Key adaptations required: (1) Add `Deserialize` derives to response types (`LLMResponse`, `AssistantMessage`, `FunctionCall`, `ResponseMetadata`, `TokenUsage`) so they work with `ctx.step()` checkpointing; (2) Remove OpenTelemetry, Lambda handler, Gemini/Bedrock transformers; (3) Remove `async-trait` dependency since `MessageTransformer` methods are actually synchronous; (4) Update `reqwest` from 0.12 to 0.13 (breaking change: feature flags renamed, rustls now default); (5) Add error classification method mapping HTTP status codes to retryable vs non-retryable for durable step retry integration; (6) Simplify `dashmap` usage in `SecretManager` -- consider replacing with a simple `RwLock<HashMap>` since per-invocation caching needs are minimal.

**Primary recommendation:** Copy the six source files verbatim, then apply targeted adaptations. The existing code structure (models.rs, service.rs, transformers/, secrets.rs, error.rs) maps cleanly to the target module layout. Focus effort on the serde adaptation for checkpoint compatibility and the error classification for retry integration.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Copy and adapt -- copy the relevant source files (models, service, transformers, secrets, error) from `~/projects/step-functions-agent/lambda/call_llm_rust/src/` into this repo. Adapt for the durable agent context (remove Lambda handler, telemetry/OpenTelemetry). Clean break from the original, no shared dependency.
- **D-02:** Code lives under `examples/src/bin/mcp_agent/` as part of the agent binary. Self-contained, no crate boundary. The module structure mirrors the original: `models.rs`, `service.rs`, `transformers/`, `secrets.rs`, `error.rs`.
- **D-03:** Include Anthropic and OpenAI transformers only. Gemini and Bedrock transformers are NOT copied for PoC -- the TransformerRegistry pattern makes them trivial to add later.
- **D-04:** Keep the same `MessageTransformer` trait and `TransformerRegistry` pattern from the original code. Register `anthropic_v1` and `openai_v1` transformers.
- **D-05:** LLM service initialization (including Secrets Manager fetch) happens per Lambda invocation, OUTSIDE durable steps. Secrets are NOT checkpointed -- they should never appear in checkpoint data.
- **D-06:** Keep the existing ProviderConfig auth model as-is (auth_header_name, auth_header_prefix, secret_path, secret_key_name). Drop-in compatible with AgentRegistry -- no schema changes needed for LLM auth.
- **D-07:** No micro-optimization needed within the agent. Agents are inherently slow (remote LLM calls + reasoning). The performance win comes from eliminating Step Functions overhead, not from optimizing internal code paths.

### Claude's Discretion
- Module structure within `examples/src/bin/mcp_agent/`
- Error type design (adapt ServiceError for durable agent needs)
- Test strategy (unit tests for transformers, integration patterns)
- Type adaptations for checkpoint serialization (ensure LLMResponse is Serialize/Deserialize for `ctx.step()` caching)
- Whether to use `async-trait` or native async traits (Rust edition 2021 with MSRV 1.88 supports async fn in traits)

### Deferred Ideas (OUT OF SCOPE)
- Gemini and Bedrock transformers -- copy from call_llm_rust when needed (v2)
- OpenTelemetry metrics for token usage and latency -- Phase 4 observability
- Streaming LLM responses -- explicitly out of scope per PROJECT.md
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LLM-01 | UnifiedLLMService extracted/adapted with provider-agnostic request/response types | Existing `service.rs` provides complete `UnifiedLLMService` with `process()` method; `models.rs` has `LLMInvocation`, `LLMResponse`, `FunctionCall`, `ResponseMetadata`. Need to add `Deserialize` to response types for checkpoint compatibility. |
| LLM-02 | Anthropic transformer for Claude models -- tool_use/tool_result content blocks | Existing `transformers/anthropic.rs` fully implements request/response mapping. Verified against current Anthropic API docs: `anthropic-version: 2023-06-01` header still current, tool_use/tool_result block format confirmed. Stop reasons include `end_turn`, `tool_use`, `max_tokens`, `stop_sequence`, `refusal`, `pause_turn`. |
| LLM-03 | OpenAI transformer for GPT models -- function calling format | Existing `transformers/openai.rs` handles tool_calls with function.arguments as JSON string, finish_reason `tool_calls`. Complex message reconstruction logic for tool_result -> tool role messages already implemented. |
| LLM-04 | Provider config matching existing ProviderConfig schema | Existing `models.rs` `ProviderConfig` struct is the exact schema from AgentRegistry. Copy as-is. Only adaptation: add `Serialize` derive for potential logging. |
| LLM-05 | API key retrieval from AWS Secrets Manager | Existing `secrets.rs` `SecretManager` with `get_api_key()` -- uses dashmap cache. Simplify to `RwLock<HashMap>` to drop dashmap dependency, or keep dashmap. Per D-05, initialization happens outside durable steps. |
| LLM-06 | LLM error classification -- retryable vs non-retryable | Existing `error.rs` has `ServiceError::ProviderApiError` with status code. Need to ADD an `is_retryable()` method. Verified retryable codes: 429 (rate_limit), 529 (overloaded), 500 (api_error), 408 (timeout). Non-retryable: 400, 401, 402, 403, 404, 413. |
| LLM-07 | Unified function_calls extraction from LLM response | Both transformers already extract `function_calls: Option<Vec<FunctionCall>>` with unified `FunctionCall { id, name, input }` struct. This is the existing design -- no changes needed. |
</phase_requirements>

## Standard Stack

### Core (from existing call_llm_rust, adapted)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `reqwest` | 0.13.2 | HTTP client for LLM API calls | Latest stable; rustls is now default (good for Lambda). Breaking change from 0.12: `json` is now an opt-in feature, `rustls-tls` renamed to `rustls` |
| `aws-sdk-secretsmanager` | 1.103.0 | Retrieve API keys from Secrets Manager | Latest on crates.io; compatible with aws-config 1.x already in use |
| `serde` / `serde_json` | 1.0 | Serialization for all types | Already in examples/Cargo.toml |
| `thiserror` | 2.0.18 | Typed error enums | Already used by SDK crate |
| `tracing` | 0.1 | Structured logging | Already in examples/Cargo.toml |

### Supporting (may or may not need)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `dashmap` | 6.1.0 | Concurrent HashMap for secret caching | Only if keeping the original SecretManager caching pattern. Alternative: `tokio::sync::RwLock<HashMap>` for simpler approach |
| `aws-config` | ~1.8.12 | AWS SDK config loading | Already a transitive dep via the SDK crate; examples may need to add it explicitly |

### NOT Needed (removed from original)

| Library | Reason for Removal |
|---------|-------------------|
| `async-trait` | `MessageTransformer` trait methods are actually synchronous (`fn`, not `async fn`). The `#[async_trait]` in the original is unnecessary. Remove it. |
| `opentelemetry` / `opentelemetry-otlp` / `tracing-opentelemetry` | Per D-01: remove telemetry. Phase 4 will add observability. |
| `lambda_runtime` (as direct dep) | Already in examples/Cargo.toml. The LLM module doesn't need it directly. |
| `base64` | Only used by Bedrock transformer (not being copied) |
| `lru` | Not used by the files being copied |
| `once_cell` | Not used; `std::sync::OnceLock` is stable since Rust 1.70 |

**Installation (additions to examples/Cargo.toml):**
```toml
# HTTP Client (for LLM API calls)
reqwest = { version = "0.13", features = ["json"], default-features = true }

# AWS SDK (for Secrets Manager)
aws-config = { version = "1.8", features = ["behavior-version-latest"] }
aws-sdk-secretsmanager = "1.103"

# Error handling (may already be available via SDK)
thiserror = "2.0"
```

**IMPORTANT: reqwest 0.13 breaking changes from 0.12:**
- `rustls-tls` feature renamed to `rustls` (and is now the default TLS backend)
- `json` feature is now opt-in (not default) -- MUST be explicitly enabled
- `default-features = false` is NO LONGER needed for rustls -- it IS the default
- `form` and `query` features are also now opt-in
- The original `Cargo.toml` had `reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }` -- this becomes `reqwest = { version = "0.13", features = ["json"] }` (simpler)

**Version verification:**
| Crate | Verified Version | Source |
|-------|-----------------|--------|
| `reqwest` | 0.13.2 | `cargo search reqwest` (2026-03-23) |
| `aws-sdk-secretsmanager` | 1.103.0 | `cargo search aws-sdk-secretsmanager` (2026-03-23) |
| `thiserror` | 2.0.18 | `cargo search thiserror` (2026-03-23) |
| `dashmap` | 6.1.0 (stable) | `cargo info dashmap@6` (2026-03-23) |

## Architecture Patterns

### Recommended Module Structure

```
examples/src/bin/mcp_agent/
  main.rs                    -- Lambda entry point (Phase 3+)
  llm/
    mod.rs                   -- pub mod declarations, re-exports
    models.rs                -- LLMInvocation, LLMResponse, ProviderConfig, UnifiedMessage, ContentBlock, etc.
    service.rs               -- UnifiedLLMService with process() method
    error.rs                 -- LlmError enum (adapted from ServiceError)
    secrets.rs               -- SecretManager with get_api_key()
    transformers/
      mod.rs                 -- MessageTransformer trait, TransformerRegistry
      anthropic.rs           -- AnthropicTransformer
      openai.rs              -- OpenAITransformer
      utils.rs               -- JSON extraction helpers (safe_extract_field, clean_tool_schema)
```

**Rationale:** Nesting LLM code under `llm/` keeps it self-contained and separates it from future modules (config, mcp, handler) that will be added in Phases 2-3. The `mod.rs` re-exports `UnifiedLLMService`, `LLMInvocation`, `LLMResponse`, `FunctionCall`, `LlmError`, and `SecretManager` for clean imports from the agent handler.

### Pattern 1: Checkpoint-Compatible Response Types

**What:** All LLM response types must derive both `Serialize` and `Deserialize` for `ctx.step()` caching.
**When to use:** Every type that appears as a return value from an LLM call step.

The existing code has response types with only `Serialize`. Must add `Deserialize`:

```rust
// BEFORE (existing code -- Serialize only)
#[derive(Debug, Clone, Serialize)]
pub struct LLMResponse { ... }

// AFTER (checkpoint-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub message: AssistantMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_calls: Option<Vec<FunctionCall>>,
    pub metadata: ResponseMetadata,
}
```

Types that need `Deserialize` added:
- `LLMResponse`
- `AssistantMessage`
- `FunctionCall`
- `ResponseMetadata`
- `TokenUsage`
- `TransformedResponse` (internal, but may flow through step boundaries)

### Pattern 2: Error Classification for Durable Retry

**What:** Map HTTP status codes to retryable/non-retryable categories so the durable step retry strategy can decide whether to retry.
**When to use:** In the error type, consumed by the agent loop when wrapping LLM calls in `ctx.step()`.

```rust
// Source: Anthropic API docs (platform.claude.com/docs/en/api/errors)
impl LlmError {
    /// Whether this error should trigger a durable step retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::ProviderApiError { status, .. } => matches!(
                status,
                429 | 500 | 502 | 503 | 529
            ),
            LlmError::HttpError(_) => true,  // Network errors are retryable
            LlmError::Timeout(_) => true,
            _ => false,  // Auth, config, transform errors are not retryable
        }
    }
}
```

Verified retryable status codes:
- **429** `rate_limit_error` -- rate limit hit, retry after backoff
- **500** `api_error` -- internal server error, transient
- **529** `overloaded_error` -- API overloaded, retry with backoff
- **502/503** -- gateway/service unavailable, transient

Non-retryable:
- **400** `invalid_request_error` -- bad request format
- **401** `authentication_error` -- bad API key
- **402** `billing_error` -- billing issue
- **403** `permission_error` -- no permission
- **404** `not_found_error` -- resource not found
- **413** `request_too_large` -- payload too large

### Pattern 3: Service Initialization Outside Durable Steps

**What:** Per D-05, `UnifiedLLMService::new()` (including Secrets Manager client creation) happens per invocation but OUTSIDE `ctx.step()`.
**When to use:** In the agent handler, before the agent loop begins.

```rust
// In the agent handler (Phase 3 integration, but the service must support this)
async fn agent_handler(event: AgentRequest, ctx: DurableContextHandle) -> DurableResult<AgentResponse> {
    // Service init: NOT checkpointed, runs every invocation
    // This is fine -- it's fast (just creates HTTP clients, no API calls yet)
    let llm_service = UnifiedLLMService::new().await
        .map_err(|e| DurableError::Internal(format!("LLM init failed: {e}")))?;

    // LLM calls: checkpointed via ctx.step()
    let response: LLMResponse = ctx.step(Some("llm-call-0"), |_| {
        let svc = llm_service.clone();  // or Arc
        let invocation = build_invocation(...);
        async move {
            svc.process(invocation).await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        }
    }, None).await?;
}
```

**Implication for UnifiedLLMService:** It must be `Clone` (or wrapped in `Arc`) to move into step closures. The existing service uses `Arc<SecretManager>` and `Arc<TransformerRegistry>` already. Adding `Clone` by wrapping `http_client` (reqwest::Client is already Clone) makes the whole service Clone-able.

### Pattern 4: Anthropic-Specific API Details (Verified)

**What:** Current Anthropic Messages API format verified against official docs (2026-03-23).
**Headers:**
- `x-api-key: {api_key}` (auth)
- `anthropic-version: 2023-06-01` (still current)
- `content-type: application/json`

**Stop reasons** (complete list as of 2026-03-23):
- `end_turn` -- model completed naturally
- `tool_use` -- model wants to call tools
- `max_tokens` -- hit the max_tokens limit
- `stop_sequence` -- hit a custom stop sequence
- `refusal` -- safety refusal (new since original code)
- `pause_turn` -- server tool iteration limit (not applicable for our use)
- `model_context_window_exceeded` -- context limit hit (new since original code)

**Tool use content block format:**
```json
{
    "type": "tool_use",
    "id": "toolu_01D7FLrfh4GYq7yT1ULFeyMV",
    "name": "get_weather",
    "input": { "location": "Paris" }
}
```

**Tool result format (in user message):**
```json
{
    "type": "tool_result",
    "tool_use_id": "toolu_01D7FLrfh4GYq7yT1ULFeyMV",
    "content": "72 degrees and sunny",
    "is_error": false
}
```

**New field since original code:** `tool_result` now supports `is_error: bool` for signaling tool execution failures. The existing `ContentBlock::ToolResult` only has `tool_use_id` and `content`. Must add `is_error: Option<bool>` for MCP error propagation (LLM-07, MCP-05 in Phase 3).

### Anti-Patterns to Avoid

- **Checkpointing secrets:** Never let API keys flow into `ctx.step()` return values. The service holds secrets in memory; only the LLM response (no keys) is checkpointed.
- **Building a custom HTTP client:** Use `reqwest` directly. The Anthropic API is a single POST endpoint -- no need for an abstraction layer beyond what `UnifiedLLMService` already provides.
- **Adding `async-trait` needlessly:** The original code has `#[async_trait]` on `MessageTransformer` but all methods are synchronous `fn`. Do not add this dependency.
- **Mixing serde and raw JSON parsing:** The existing transformers use `serde_json::Value` for request building and response parsing. This is fine for a transformer pattern where the wire format varies by provider. Don't try to fully type the provider-specific formats -- the unified types (`LLMResponse`, `FunctionCall`) are the typed boundary.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Secret caching | Custom cache with TTL eviction | `DashMap` or `RwLock<HashMap>` with `Instant`-based TTL | Existing pattern in `secrets.rs` handles cache invalidation, concurrent access, and JSON parsing |
| HTTP client with retry | Custom retry loop around reqwest | Durable SDK `ctx.step()` with `ExponentialBackoff` retry strategy | The SDK already provides retry with backoff/jitter; LLM calls inside steps get free retry |
| JSON path extraction | Custom JSON traversal | `transformers/utils.rs` helpers (`safe_extract_field`, `extract_with_fallback`) | Already handles dot-notation paths, array indices, fallback chains |
| Tool schema cleaning | Manual JSON Schema normalization | `clean_tool_schema()` in utils.rs | Handles edge cases: missing properties, empty required arrays, non-object schemas |

**Key insight:** The existing call_llm_rust code already solves all the fiddly problems (OpenAI message reconstruction for tool results, Anthropic system prompt extraction, JSON schema cleaning). Copy it; don't reinvent it.

## Common Pitfalls

### Pitfall 1: Missing Deserialize on Response Types
**What goes wrong:** `ctx.step()` returns cached `LLMResponse` on replay, which requires `DeserializeOwned`. Without `Deserialize`, compilation fails when the LLM call step is wired into the agent loop.
**Why it happens:** The original code only needed `Serialize` (to return JSON from a Lambda). The durable context needs round-trip serialization.
**How to avoid:** Add `#[derive(Deserialize)]` to ALL response types: `LLMResponse`, `AssistantMessage`, `FunctionCall`, `ResponseMetadata`, `TokenUsage`. Also ensure `ContentBlock` (which already has both) maintains them.
**Warning signs:** Compilation error mentioning `DeserializeOwned` bound not satisfied.

### Pitfall 2: reqwest 0.13 Feature Flag Changes
**What goes wrong:** Build fails with "no method named `json` found" or uses OpenSSL instead of rustls.
**Why it happens:** reqwest 0.13 changed feature flags -- `json` is now opt-in, `rustls-tls` was renamed to `rustls` (and is default).
**How to avoid:** Use `reqwest = { version = "0.13", features = ["json"] }`. Do NOT use `default-features = false` (that would disable rustls, the opposite of what you want).
**Warning signs:** Missing method errors at compile time; OpenSSL linking errors on Lambda.

### Pitfall 3: ServiceError Cannot Be Used as step() Error
**What goes wrong:** `ctx.step()` expects `Result<T, Box<dyn Error + Send + Sync>>`. `ServiceError` must implement `std::error::Error + Send + Sync`.
**Why it happens:** `thiserror` 2.x `#[derive(Error)]` automatically implements `std::error::Error`. But `ServiceError` contains `reqwest::Error` and `aws_sdk_secretsmanager::Error` which must also be `Send + Sync`.
**How to avoid:** Verify that `reqwest::Error` and AWS SDK errors are `Send + Sync` (they are). The existing `#[from]` conversions handle this. Just ensure `LlmError` (renamed from `ServiceError`) also derives `Send + Sync` compatible traits.
**Warning signs:** Trait bound errors mentioning `Send` or `Sync` not satisfied.

### Pitfall 4: Empty Anthropic Responses After Tool Results
**What goes wrong:** Claude returns empty response with `stop_reason: "end_turn"` after receiving tool results.
**Why it happens:** Adding text blocks immediately after tool_result in the user message teaches Claude to expect user input after every tool use, causing it to end its turn.
**How to avoid:** Send tool_result blocks alone in the user message, without additional text blocks. The existing transformer already does this correctly.
**Warning signs:** LLM returns empty content array with `end_turn` during the agent loop.

### Pitfall 5: OpenAI Tool Call Argument Parsing
**What goes wrong:** Tool arguments come as a JSON string from OpenAI (`"arguments": "{\"key\": \"value\"}"`), not as a parsed object.
**Why it happens:** OpenAI API returns function arguments as stringified JSON, unlike Anthropic which returns parsed objects.
**How to avoid:** The existing `OpenAITransformer::extract_tool_calls()` already handles this with `serde_json::from_str(args_str).unwrap_or(json!({}))`. Copy this logic faithfully.
**Warning signs:** Tool input is a string instead of a JSON object.

### Pitfall 6: aws-sdk-secretsmanager Error Type Change
**What goes wrong:** The existing code uses `aws_sdk_secretsmanager::Error` for the `#[from]` conversion. The AWS SDK error types may have changed between the version in call_llm_rust (1.49) and current (1.103).
**Why it happens:** AWS SDK for Rust frequently updates error type hierarchies.
**How to avoid:** Use `Box<dyn std::error::Error + Send + Sync>` for the AWS SDK error variant, or verify the exact error type at the current version. The `SdkError` from `aws_sdk_secretsmanager::operation::get_secret_value` is the primary error path.
**Warning signs:** `From` trait not implemented errors during compilation.

## Code Examples

### Adapted UnifiedLLMService (sketch)

```rust
// Source: ~/projects/step-functions-agent/lambda/call_llm_rust/src/service.rs
// Adaptation: Clone support, Deserialize on responses, error classification

use std::sync::Arc;

#[derive(Clone)]
pub struct UnifiedLLMService {
    secret_manager: Arc<SecretManager>,
    http_client: reqwest::Client,  // reqwest::Client is already Clone
    transformer_registry: Arc<TransformerRegistry>,
}

impl UnifiedLLMService {
    pub async fn new() -> Result<Self, LlmError> {
        let secret_manager = Arc::new(SecretManager::new().await?);

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))  // LLM calls can be slow
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .build()
            .map_err(LlmError::HttpError)?;

        let transformer_registry = Arc::new(TransformerRegistry::new());

        Ok(Self { secret_manager, http_client, transformer_registry })
    }

    pub async fn process(&self, invocation: LLMInvocation) -> Result<LLMResponse, LlmError> {
        // Same flow as original: get key -> transform request -> call provider -> transform response
        // ... (copy from service.rs with minimal changes)
    }
}
```

### Adapted Error Type with Classification

```rust
// Source: ~/projects/step-functions-agent/lambda/call_llm_rust/src/error.rs
// Adaptation: is_retryable() method, removed LambdaError variant

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LlmError {
    #[error("Transformer not found: {0}")]
    TransformerNotFound(String),

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Secret key not found: {0} in secret: {1}")]
    SecretKeyNotFound(String, String),

    #[error("AWS SDK error: {0}")]
    AwsSdkError(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Provider API error: {provider} returned {status}: {message}")]
    ProviderApiError {
        provider: String,
        status: u16,
        message: String,
    },

    #[error("Transform error: {0}")]
    TransformError(String),
}

impl LlmError {
    /// Whether this error is transient and should be retried by the durable step.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ProviderApiError { status, .. } => {
                matches!(status, 429 | 500 | 502 | 503 | 529)
            }
            Self::HttpError(e) => e.is_timeout() || e.is_connect(),
            _ => false,
        }
    }
}
```

### Adapted ContentBlock with is_error

```rust
// Source: ~/projects/step-functions-agent/lambda/call_llm_rust/src/models.rs
// Adaptation: Added is_error to ToolResult variant for MCP error propagation

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    #[serde(rename = "image")]
    Image {
        source: ImageSource,
    },
}
```

### TransformerRegistry Without async-trait

```rust
// Source: ~/projects/step-functions-agent/lambda/call_llm_rust/src/transformers/mod.rs
// Adaptation: Removed #[async_trait], only register anthropic_v1 and openai_v1

pub trait MessageTransformer: Send + Sync {
    fn transform_request(&self, invocation: &LLMInvocation) -> Result<TransformedRequest, LlmError>;
    fn transform_response(&self, response: Value) -> Result<TransformedResponse, LlmError>;
    fn get_headers(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

pub struct TransformerRegistry {
    transformers: HashMap<String, Box<dyn MessageTransformer>>,
}

impl TransformerRegistry {
    pub fn new() -> Self {
        let mut transformers: HashMap<String, Box<dyn MessageTransformer>> = HashMap::new();
        transformers.insert("openai_v1".to_string(), Box::new(OpenAITransformer));
        transformers.insert("anthropic_v1".to_string(), Box::new(AnthropicTransformer));
        Self { transformers }
    }

    pub fn get(&self, name: &str) -> Result<&dyn MessageTransformer, LlmError> {
        self.transformers
            .get(name)
            .map(|t| t.as_ref())
            .ok_or_else(|| LlmError::TransformerNotFound(name.to_string()))
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| reqwest 0.12 with `rustls-tls` feature | reqwest 0.13 with rustls as default | Jan 2026 | Simpler Cargo.toml; `json` feature must be explicit |
| `async-trait` for all trait objects | Native async fn in traits (Rust 1.75+) | Dec 2023 | Not applicable here since MessageTransformer methods are sync, but `async-trait` is no longer needed even conceptually |
| Anthropic `stop_reason` only had 4 values | Now includes `refusal`, `pause_turn`, `model_context_window_exceeded` | 2025-2026 | Agent loop should handle new stop reasons gracefully |
| Anthropic `tool_result` had no `is_error` | Now includes `is_error: bool` field | 2024 | Important for MCP error propagation in Phase 3 |
| `dashmap` 6.1 stable | `dashmap` 7.0.0-rc2 exists but is RC | 2026 | Use 6.1 stable; 7.0 is not ready |

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | cargo test (built-in) with `#[tokio::test]` for async |
| Config file | none (uses Cargo.toml `[dev-dependencies]`) |
| Quick run command | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| Full suite command | `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -- -D warnings` |

### Phase Requirements -> Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LLM-01 | UnifiedLLMService process() returns LLMResponse | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::service` | Wave 0 |
| LLM-02 | AnthropicTransformer transforms request/response with tool_use blocks | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::transformers::anthropic` | Wave 0 |
| LLM-03 | OpenAITransformer transforms request/response with function calling | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::transformers::openai` | Wave 0 |
| LLM-04 | ProviderConfig deserializes from JSON matching AgentRegistry schema | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::models` | Wave 0 |
| LLM-05 | SecretManager retrieves API key from Secrets Manager | unit (mocked) | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::secrets` | Wave 0 |
| LLM-06 | Error classification: retryable vs non-retryable | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::error` | Wave 0 |
| LLM-07 | function_calls extraction from both Anthropic and OpenAI responses | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib llm::transformers` | Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --manifest-path examples/Cargo.toml --all-targets`
- **Per wave merge:** `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -- -D warnings && cargo fmt --manifest-path examples/Cargo.toml --check`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] No test infrastructure exists for example binaries -- tests will live as `#[cfg(test)] mod tests` within the LLM module files
- [ ] Examples Cargo.toml needs `autobins = false` to keep multi-file bins working (already set)
- [ ] May need `serde_json` in dev-dependencies for test fixture construction (already a regular dependency)
- [ ] Test helper for creating mock `ProviderConfig`, `LLMInvocation`, and provider response JSON fixtures

**Note on test architecture:** Since the code lives in a binary crate (`examples/src/bin/mcp_agent/`), unit tests must be `#[cfg(test)] mod tests` within the module files. The examples Cargo.toml `--all-targets` flag will pick these up. Integration tests requiring real AWS services are out of scope for Phase 1 validation.

## Open Questions

1. **Binary crate test visibility**
   - What we know: Multi-file binaries under `src/bin/mcp_agent/` can have inline tests. `cargo test --all-targets` runs them.
   - What's unclear: Whether the examples workspace test runner correctly picks up tests in deeply nested binary modules (`src/bin/mcp_agent/llm/transformers/anthropic.rs`).
   - Recommendation: Verify early with a trivial test. If tests aren't discovered, consider a `lib.rs` approach where the agent modules are a library target consumed by the `main.rs` binary.

2. **SecretManager caching strategy**
   - What we know: Original uses `dashmap` 6.1 with 5-minute TTL. Per D-05, secrets are fetched per invocation outside durable steps.
   - What's unclear: Whether Lambda function instances live long enough for the 5-minute cache to provide benefit within a single invocation.
   - Recommendation: Start with `dashmap` (copy as-is). Simplify to `RwLock<HashMap>` only if dashmap causes dependency conflicts. The caching is harmless even if unused.

3. **ProviderConfig Serialize derive**
   - What we know: Original `ProviderConfig` only derives `Deserialize`. It's input-only in the original Lambda.
   - What's unclear: Whether the durable agent needs to serialize `ProviderConfig` (e.g., as part of a step result or for logging).
   - Recommendation: Add `Serialize` derive preemptively. It costs nothing and avoids surprises.

## Sources

### Primary (HIGH confidence)
- `~/projects/step-functions-agent/lambda/call_llm_rust/src/` -- all 7 source files read and analyzed
- `examples/Cargo.toml` -- current examples dependency configuration
- `src/context/durable_context/step.rs` -- step() generic bounds (`T: Serialize + DeserializeOwned`)
- [Anthropic Messages API](https://platform.claude.com/docs/en/api/messages) -- request/response format, headers
- [Anthropic Error Codes](https://platform.claude.com/docs/en/api/errors) -- all HTTP error types and retryability
- [Anthropic Stop Reasons](https://platform.claude.com/docs/en/api/handling-stop-reasons) -- complete stop_reason enum
- `cargo search` / `cargo info` -- verified crate versions (2026-03-23)

### Secondary (MEDIUM confidence)
- [reqwest v0.13 release blog](https://seanmonstar.com/blog/reqwest-v013-rustls-default/) -- breaking changes from 0.12
- [OpenAI Function Calling docs](https://platform.openai.com/docs/guides/function-calling) -- tool_calls response format (verified via web search)
- [Rust async fn in traits stabilization](https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits.html) -- native async trait support (confirmed MessageTransformer doesn't need it)

### Tertiary (LOW confidence)
- OpenAI exact response JSON field names -- based on existing transformer code and training data; the existing transformer works in production so this is implicitly verified

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- crate versions verified against crates.io; existing code is production-tested
- Architecture: HIGH -- module structure mirrors proven existing code; checkpoint compatibility requirements verified against SDK source
- Pitfalls: HIGH -- identified from actual API docs and version change analysis; error classification verified against Anthropic docs
- Validation: MEDIUM -- binary crate testing in workspace needs early verification

**Research date:** 2026-03-23
**Valid until:** 2026-04-23 (stable domain; APIs and crate versions change slowly)
