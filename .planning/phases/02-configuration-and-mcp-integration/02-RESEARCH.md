# Phase 2: Configuration and MCP Integration - Research

**Researched:** 2026-03-23
**Domain:** DynamoDB config loading + MCP client integration (pmcp SDK)
**Confidence:** HIGH

## Summary

This phase has two domains: (1) loading agent configuration from the existing AgentRegistry DynamoDB table and mapping it to internal types, and (2) connecting to MCP servers using the local pmcp SDK to discover tools and translate their schemas to the unified tool format used by the LLM client.

The DynamoDB schema is well-understood from the existing Python implementation. The table uses `agent_name` (PK) + `version` (SK) with fields stored as DynamoDB strings (some JSON-encoded). The Rust implementation reads these fields and maps `llm_provider` + `llm_model` to a `ProviderConfig` via a hardcoded mapping table.

The pmcp SDK is a local path dependency (v2.0.0 at `~/Development/mcp/sdk/rust-mcp-sdk/`). The crates.io version is only 1.20.0, confirming the path dependency approach. The SDK provides `StreamableHttpTransport` (with TLS via rustls/ring) and `Client<T: Transport>` with `initialize()`, `list_tools()`, and `call_tool()` methods. Tool schemas come as `ToolInfo` structs with `name`, `description`, and `input_schema` (JSON Schema `Value`) which map directly to the existing `UnifiedTool` type.

**Primary recommendation:** Use `StreamableHttpTransport` (not `HttpTransport` which lacks TLS), add pmcp as a path dependency with features `["streamable-http"]`, and implement both config and MCP modules as new peer modules alongside the existing `llm/` module.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** The `mcp_servers` field is a simple URL array: `["https://mcp1.example.com", "https://mcp2.example.com"]`. No per-server auth or metadata in DynamoDB. Simplest possible schema extension.
- **D-02:** Read existing `llm_provider` + `llm_model` fields from AgentRegistry (not a full ProviderConfig). Map to ProviderConfig in code (provider -> endpoint, transformer ID, secret path). This preserves the operator workflow where different steps update different records (providers, API keys, model enablement, agent configuration).
- **D-03:** Config loading is a durable `ctx.step()` -- the DynamoDB read result is cached on replay.
- **D-04:** MCP servers are deployed as Lambda functions with Web Adapter behind HTTP endpoints (API Gateway or Function URLs). The agent uses pmcp HttpTransport to connect.
- **D-05:** No application-level auth for PoC -- servers are accessed via IAM/VPC. OAuth support deferred to later using existing PMCP SDK OAuth capabilities (user-forwarded tokens or M2M tokens).
- **D-06:** Expect 1-3 MCP servers per agent. Sequential connection is fine -- total setup under 1 second. No need for parallel connection logic.
- **D-07:** MCP connections are ephemeral -- established per Lambda invocation, NOT inside durable steps. Only the results (tool schemas, tool call outputs) are checkpointed. Connections cannot be serialized.
- **D-08:** Prefix all tool names with a host-based identifier extracted from the MCP server URL. Format: `{host_prefix}__{tool_name}` (e.g., `calc__multiply`, `wiki__search`). This applies to all tools regardless of whether collisions exist -- consistent naming for the LLM.
- **D-09:** When routing a tool call back to the correct MCP server, reverse-map the prefix to find the originating server URL.

### Claude's Discretion
- DynamoDB client setup and query implementation
- MCP schema-to-Claude API tool format translation (straightforward field mapping)
- Error types for config loading failures and MCP connection failures
- How to extract a short host prefix from URLs (e.g., `calc-server.us-east-1.amazonaws.com` -> `calc-server`)
- Test strategy (mock DynamoDB, mock MCP server responses)

### Deferred Ideas (OUT OF SCOPE)
- OAuth/token-based MCP server authentication -- future phase, using PMCP SDK OAuth capabilities
- Parallel MCP server connection -- not needed for 1-3 servers
- MCP server health checks / circuit breaker -- future hardening
- Dynamic provider config from a separate DynamoDB table -- current approach maps in code
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CONF-01 | Agent reads configuration from AgentRegistry DynamoDB table by agent_name and version | DynamoDB schema fully documented from Python source; `get_item` with PK/SK pattern verified |
| CONF-02 | Configuration includes system_prompt, llm_model, temperature, max_tokens, max_iterations | All fields present in existing DynamoDB schema as strings or JSON-encoded `parameters` map |
| CONF-03 | Configuration includes mcp_servers array with endpoint URLs | Additive field to existing schema; stored as JSON string array in DynamoDB (per D-01) |
| CONF-04 | Config loading is a durable `ctx.step()` -- cached on replay | `ctx.step()` API verified: returns `DurableResult<T>` where `T: Serialize + DeserializeOwned`, config struct must derive both |
| MCP-01 | Agent connects to configured MCP servers via pmcp HttpTransport and initializes each connection | `StreamableHttpTransport` with TLS verified in pmcp source; `Client::new()` + `client.initialize(ClientCapabilities::default())` pattern confirmed |
| MCP-02 | Agent discovers tools from each MCP server via `list_tools()` and merges into a unified tool list | `client.list_tools(None)` returns `ListToolsResult { tools: Vec<ToolInfo> }`; pagination via cursor supported but unlikely needed for small tool sets |
| MCP-03 | MCP tool schemas translated to Claude API tool format (name, description, input_schema) | `ToolInfo { name, description: Option<String>, input_schema: Value }` maps 1:1 to `UnifiedTool { name, description, input_schema }` with `clean_tool_schema()` normalization |
| MCP-06 | MCP connection failure at startup fails fast with clear error | Error types in pmcp (`Error::Transport`, `Error::InvalidState`) propagate cleanly; agent should fail before LLM call if any MCP server is unreachable |
</phase_requirements>

## Standard Stack

### Core (New Dependencies for Phase 2)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `aws-sdk-dynamodb` | 1.110.0 | Read AgentRegistry config | Official AWS SDK; consistent with existing aws-sdk-secretsmanager pattern |
| `pmcp` | 2.0.0 (path dep) | MCP client (list_tools, call_tool) | Local development SDK at ~/Development/mcp/sdk/rust-mcp-sdk/ |
| `url` | 2.5 | Parse MCP server URLs, extract host prefix | Transitive via pmcp; use directly for URL parsing |

### Already Available (from Phase 1)
| Library | Version | Purpose |
|---------|---------|---------|
| `serde` / `serde_json` | 1.0 | Serialize config types for checkpoint |
| `aws-config` | 1.8 | AWS SDK config loading |
| `thiserror` | 2.0 | Error type definitions |
| `tracing` | 0.1 | Structured logging |
| `reqwest` | 0.13 | Already in deps (used by LLM client) |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Raw `aws-sdk-dynamodb` | `serde_dynamo` (v4.3.0) | serde_dynamo provides cleaner attribute-to-struct mapping, but adds a dependency for ~20 lines of parsing code. Manual parsing is simple enough for the 8 fields we read. |
| `StreamableHttpTransport` | `HttpTransport` | HttpTransport uses bare hyper without TLS (HTTP only). MCP servers are on HTTPS endpoints. StreamableHttpTransport includes rustls with ring crypto provider, explicitly compatible with Lambda. |
| pmcp path dep | pmcp crates.io (v1.20.0) | crates.io is behind at v1.20.0. Local dev copy is v2.0.0 with StreamableHttpTransport API we need. Path dependency required. |

**Installation (additions to examples/Cargo.toml):**
```toml
# DynamoDB
aws-sdk-dynamodb = "1.110"

# MCP Client (local path to pmcp SDK)
pmcp = { path = "../../sdk/rust-mcp-sdk", features = ["streamable-http"] }
url = "2.5"
```

**Version verification:**
- `aws-sdk-dynamodb`: verified 1.110.0 on crates.io (2026-03-23)
- `pmcp`: v2.0.0 from local path (crates.io has 1.20.0 -- path dep required)
- `url`: 2.5 (transitive via pmcp, but adding directly for explicit use)

## Architecture Patterns

### Recommended Module Structure
```
examples/src/bin/mcp_agent/
  main.rs           -- Lambda entry point (exists)
  llm/              -- LLM client module (exists from Phase 1)
    mod.rs
    models.rs       -- ProviderConfig, UnifiedTool, etc.
    service.rs      -- UnifiedLLMService
    ...
  config/           -- NEW: Configuration loading
    mod.rs          -- Re-exports
    types.rs        -- AgentConfig, AgentParameters structs
    loader.rs       -- DynamoDB reader, provider mapping
    error.rs        -- ConfigError enum
  mcp/              -- NEW: MCP client integration
    mod.rs          -- Re-exports
    client.rs       -- McpClientManager: connect, discover, translate
    types.rs        -- ToolsWithRouting, McpServerInfo
    error.rs        -- McpError enum
```

### Pattern 1: DynamoDB Config Loading

**What:** Read agent configuration from DynamoDB and map to internal types.
**When to use:** Called once per execution inside `ctx.step("load-config")`.

```rust
// Source: agent_registry.py analysis + aws-sdk-dynamodb API
use aws_sdk_dynamodb::Client as DdbClient;
use aws_sdk_dynamodb::types::AttributeValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_name: String,
    pub version: String,
    pub system_prompt: String,
    pub provider_config: ProviderConfig,
    pub mcp_server_urls: Vec<String>,
    pub parameters: AgentParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentParameters {
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout_seconds: u32,
}

pub async fn load_agent_config(
    table_name: &str,
    agent_name: &str,
    version: &str,
) -> Result<AgentConfig, Box<dyn std::error::Error + Send + Sync>> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = DdbClient::new(&config);

    let result = client.get_item()
        .table_name(table_name)
        .key("agent_name", AttributeValue::S(agent_name.to_string()))
        .key("version", AttributeValue::S(version.to_string()))
        .send()
        .await?;

    let item = result.item()
        .ok_or("Agent not found in registry")?;

    parse_agent_config(item)
}
```

**DynamoDB field mapping (from Python source analysis):**

| DynamoDB Field | DynamoDB Type | Rust Target | Notes |
|---------------|---------------|-------------|-------|
| `agent_name` | S (String) | `String` | PK |
| `version` | S (String) | `String` | SK |
| `system_prompt` | S (String) | `String` | Direct |
| `llm_provider` | S (String) | Input to provider mapping | e.g. "claude", "openai", "gemini" |
| `llm_model` | S (String) | Input to provider mapping | e.g. "claude-3-5-sonnet-20241022" |
| `parameters` | S (JSON string) | `AgentParameters` | JSON-encoded: `{"max_iterations": 5, "temperature": 0.3, ...}` |
| `mcp_servers` | S (JSON string) | `Vec<String>` | JSON-encoded URL array (new additive field) |
| `tools` | S (JSON string) | Ignored for MCP agent | Legacy Step Functions tool refs |
| `status` | S (String) | Optional read | "active", "deprecated", "testing" |

### Pattern 2: Provider Mapping (llm_provider + llm_model -> ProviderConfig)

**What:** Hardcoded mapping from DynamoDB fields to full ProviderConfig. Per D-02, the operator sets `llm_provider` and `llm_model` in the registry. The code maps these to the full configuration including endpoint, auth, and transformer IDs.

```rust
// Source: existing ProviderConfig in models.rs + agent_registry.py patterns
pub fn map_provider_config(
    llm_provider: &str,
    llm_model: &str,
) -> Result<ProviderConfig, ConfigError> {
    match llm_provider {
        "claude" | "anthropic" => Ok(ProviderConfig {
            provider_id: "anthropic".to_string(),
            model_id: llm_model.to_string(),
            endpoint: "https://api.anthropic.com/v1/messages".to_string(),
            auth_header_name: "x-api-key".to_string(),
            auth_header_prefix: None,
            secret_path: "prod/anthropic/api-key".to_string(),
            secret_key_name: "api_key".to_string(),
            request_transformer: "anthropic_v1".to_string(),
            response_transformer: "anthropic_v1".to_string(),
            timeout: 120,
            custom_headers: Some(HashMap::from([
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ])),
        }),
        "openai" => Ok(ProviderConfig {
            provider_id: "openai".to_string(),
            model_id: llm_model.to_string(),
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            auth_header_name: "Authorization".to_string(),
            auth_header_prefix: Some("Bearer ".to_string()),
            secret_path: "prod/openai/api-key".to_string(),
            secret_key_name: "api_key".to_string(),
            request_transformer: "openai_v1".to_string(),
            response_transformer: "openai_v1".to_string(),
            timeout: 120,
            custom_headers: None,
        }),
        _ => Err(ConfigError::UnsupportedProvider(llm_provider.to_string())),
    }
}
```

### Pattern 3: MCP Tool Discovery with Prefix and Routing

**What:** Connect to each MCP server, discover tools, prefix names, build routing map.
**When to use:** Called once per execution inside `ctx.step("discover-tools")`. The connections are ephemeral (D-07) but the results (tool schemas + routing) are serializable and checkpointed.

```rust
// Source: pmcp Client API (client/mod.rs), ToolInfo (types/tools.rs)
use pmcp::{Client, ClientCapabilities, Implementation};
use pmcp::shared::streamable_http::{
    StreamableHttpTransport, StreamableHttpTransportConfig,
};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsWithRouting {
    pub tools: Vec<UnifiedTool>,
    pub routing: HashMap<String, String>,  // prefixed_tool_name -> server_url
}

pub async fn discover_tools(
    server_urls: &[String],
) -> Result<ToolsWithRouting, Box<dyn std::error::Error + Send + Sync>> {
    let mut tools = Vec::new();
    let mut routing = HashMap::new();

    for server_url in server_urls {
        let prefix = extract_host_prefix(server_url)?;

        // Create transport with TLS support
        let url = Url::parse(server_url)?;
        let config = StreamableHttpTransportConfig {
            url: url.clone(),
            extra_headers: vec![],
            auth_provider: None,
            session_id: None,
            enable_json_response: false,
            on_resumption_token: None,
            http_middleware_chain: None,
        };
        let transport = StreamableHttpTransport::new(config);

        // Initialize MCP client
        let mut client = Client::new(transport);
        client.initialize(ClientCapabilities::default()).await?;

        // Discover tools
        let result = client.list_tools(None).await?;

        for tool_info in result.tools {
            let prefixed_name = format!("{prefix}__{}", tool_info.name);
            let unified_tool = UnifiedTool {
                name: prefixed_name.clone(),
                description: tool_info.description.unwrap_or_default(),
                input_schema: clean_tool_schema(&tool_info.input_schema),
            };
            routing.insert(prefixed_name, server_url.clone());
            tools.push(unified_tool);
        }
    }

    Ok(ToolsWithRouting { tools, routing })
}

/// Extract a short host prefix from an MCP server URL.
/// "https://calc-server.us-east-1.amazonaws.com/mcp" -> "calc-server"
/// "https://wiki-tool.example.com" -> "wiki-tool"
fn extract_host_prefix(server_url: &str) -> Result<String, ConfigError> {
    let url = Url::parse(server_url)?;
    let host = url.host_str().ok_or(ConfigError::InvalidUrl(server_url.to_string()))?;
    // Take the first segment before the first dot
    let prefix = host.split('.').next().unwrap_or(host);
    Ok(prefix.to_string())
}
```

### Pattern 4: Reverse-Mapping Tool Calls to MCP Servers

**What:** When the LLM returns a tool_use block with prefixed name, strip the prefix and route to the correct server.
**When to use:** Phase 3 (agent loop) will use this, but the data structure is built in Phase 2.

```rust
/// Given a prefixed tool name (e.g., "calc__multiply"), return the
/// server URL and original tool name ("multiply").
pub fn resolve_tool_call(
    prefixed_name: &str,
    routing: &HashMap<String, String>,
) -> Result<(String, String), McpError> {
    let server_url = routing.get(prefixed_name)
        .ok_or_else(|| McpError::UnknownTool(prefixed_name.to_string()))?;

    // Strip prefix: "calc__multiply" -> "multiply"
    let original_name = prefixed_name
        .split("__")
        .nth(1)
        .ok_or_else(|| McpError::InvalidToolName(prefixed_name.to_string()))?
        .to_string();

    Ok((server_url.clone(), original_name))
}
```

### Anti-Patterns to Avoid

- **Creating DynamoDB client inside the step closure:** The aws-config loading and DynamoDB client creation adds latency. Create the client outside the step and clone into the closure. However, DO put the actual `get_item` call inside the step so results are checkpointed. The client creation is deterministic so it's safe outside.

- **Storing MCP `Client` in agent state:** The `Client<StreamableHttpTransport>` holds live TCP connections and cannot be serialized. Only store serializable results (ToolsWithRouting).

- **Using `HttpTransport` instead of `StreamableHttpTransport`:** The `HttpTransport` in pmcp uses bare hyper `HttpConnector` without TLS. MCP servers behind API Gateway or Lambda Function URLs use HTTPS. `StreamableHttpTransport` includes hyper-rustls with ring crypto provider and explicitly documents Lambda compatibility.

- **Parsing DynamoDB JSON strings with `serde_json::from_str` without error handling:** DynamoDB `parameters` field is operator-edited JSON. Validate and provide defaults for missing fields.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| DynamoDB access | Custom HTTP calls to DynamoDB | `aws-sdk-dynamodb` | Handles SigV4 auth, retries, marshaling |
| MCP protocol handshake | Custom JSON-RPC over HTTP | `pmcp` Client + StreamableHttpTransport | MCP protocol has session management, capability negotiation, SSE parsing |
| TLS for MCP connections | OpenSSL linking or custom TLS | pmcp's built-in rustls/ring via StreamableHttpTransport | ring crypto provider avoids aws-lc-rs conflicts in Lambda |
| URL parsing and host extraction | Regex on URLs | `url::Url` crate | Handles edge cases (ports, paths, encoded chars) |
| JSON Schema normalization | Custom schema walker | Existing `clean_tool_schema()` in utils.rs | Already handles type enforcement, empty required arrays, standard field passthrough |

**Key insight:** The tool schema translation is deceptively simple because both MCP and Claude use JSON Schema for tool input parameters. The `ToolInfo.input_schema` maps directly to `UnifiedTool.input_schema`. The only transformation needed is `clean_tool_schema()` normalization (already implemented) and the name prefixing (per D-08).

## Common Pitfalls

### Pitfall 1: pmcp Feature Flags Missing
**What goes wrong:** Compiling pmcp without `streamable-http` feature results in `StreamableHttpTransport` not being available. The `HttpTransport` is gated behind `http` feature (which also lacks TLS).
**Why it happens:** pmcp's default features are `["logging"]` only. Transport implementations are opt-in.
**How to avoid:** Specify `features = ["streamable-http"]` in Cargo.toml. This pulls in hyper, hyper-util, hyper-rustls, rustls, axum, tower, tower-http, futures-util, and bytes.
**Warning signs:** Compile error "cannot find struct `StreamableHttpTransport`" or "unresolved import `pmcp::shared::streamable_http`".

### Pitfall 2: DynamoDB String Attributes Containing JSON
**What goes wrong:** The `parameters` and `mcp_servers` fields are stored as DynamoDB `S` (String) type containing JSON. Attempting to read them as DynamoDB `L` (List) or `M` (Map) types fails with missing attribute errors.
**Why it happens:** The existing Python code stores complex types as JSON strings (see `_parse_agent_item` which calls `json.loads`), not as native DynamoDB types.
**How to avoid:** Read as `AttributeValue::S`, then `serde_json::from_str()` to parse the inner JSON.
**Warning signs:** `None` when calling `.as_l()` or `.as_m()` on what you expected to be a list/map.

### Pitfall 3: MCP Client Requires Initialize Before list_tools
**What goes wrong:** Calling `client.list_tools()` before `client.initialize()` returns `Error::InvalidState("Client not initialized")`.
**Why it happens:** MCP protocol requires an initialization handshake (capabilities exchange) before any operations. The pmcp Client enforces this with `ensure_initialized()` checks on every method.
**How to avoid:** Always call `client.initialize(ClientCapabilities::default()).await?` after creating the client and before any operations.
**Warning signs:** `Error::InvalidState` errors at runtime.

### Pitfall 4: Tool Name Prefix Collision with Double Underscore
**What goes wrong:** If a tool name already contains `__`, the reverse-mapping logic that splits on `__` to find the prefix and original name breaks.
**Why it happens:** Using `split("__").nth(1)` only gets the segment after the first `__`. If the original tool name is `my__tool`, the split would yield `["calc", "my", "tool"]`.
**How to avoid:** Use `splitn(2, "__")` to split into exactly 2 parts at the first occurrence. This preserves `__` in original tool names.
**Warning signs:** Tool call routing failures for tools with underscores in their names.

### Pitfall 5: ring vs aws-lc-rs Crypto Provider Conflict
**What goes wrong:** The AWS SDK uses aws-lc-rs by default. pmcp's StreamableHttpTransport uses rustls with ring. If both try to register as the default crypto provider, one fails.
**Why it happens:** rustls requires exactly one crypto provider installed. Both ring and aws-lc-rs implement the same trait.
**How to avoid:** The pmcp StreamableHttpTransport already handles this: `let _ = rustls::crypto::ring::default_provider().install_default();` is idempotent and runs before creating the HTTPS connector. The AWS SDK uses its own TLS stack (not rustls), so there's no actual conflict. However, if the project has other dependencies using rustls with aws-lc-rs, there could be a conflict. Test early.
**Warning signs:** Panic at runtime: "cannot install default CryptoProvider".

### Pitfall 6: Config Step Return Type Must Be Serializable
**What goes wrong:** `ctx.step()` requires `T: Serialize + DeserializeOwned`. If `AgentConfig` contains non-serializable fields (like an `aws_sdk_dynamodb::Client` handle), the step fails at checkpoint time.
**Why it happens:** Durable steps checkpoint their return value as JSON for replay.
**How to avoid:** `AgentConfig` must contain only data types (strings, numbers, Vec, HashMap). No client handles, no connection objects, no Arc/Mutex. All `derive(Serialize, Deserialize)`.
**Warning signs:** Compile error on `ctx.step()` generic bounds, or runtime serialization panic.

## Code Examples

### DynamoDB Attribute Extraction Helpers

```rust
// Source: aws-sdk-dynamodb API patterns
use aws_sdk_dynamodb::types::AttributeValue;

fn get_string(item: &HashMap<String, AttributeValue>, key: &str) -> Result<String, ConfigError> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| ConfigError::MissingField(key.to_string()))
}

fn get_json_string_as<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    key: &str,
) -> Result<T, ConfigError> {
    let json_str = get_string(item, key)?;
    serde_json::from_str(&json_str)
        .map_err(|e| ConfigError::InvalidJson { field: key.to_string(), source: e })
}

fn get_optional_json_string_as<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    key: &str,
    default: T,
) -> T {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(default)
}
```

### Full MCP Client Connection Lifecycle

```rust
// Source: pmcp client/mod.rs Client::initialize + Client::list_tools
use pmcp::{Client, ClientCapabilities, Implementation};
use pmcp::shared::streamable_http::{
    StreamableHttpTransport,
    StreamableHttpTransportConfig,
    StreamableHttpTransportConfigBuilder,
};

async fn connect_and_discover(
    server_url: &str,
) -> Result<Vec<pmcp::types::ToolInfo>, Box<dyn std::error::Error + Send + Sync>> {
    let url = url::Url::parse(server_url)?;

    // Use the builder for cleaner config
    let config = StreamableHttpTransportConfigBuilder::new(url)
        .enable_json_response()  // JSON instead of SSE for simple request/response
        .build();
    let transport = StreamableHttpTransport::new(config);

    // Create and initialize client
    let mut client = Client::with_info(
        transport,
        Implementation::new("durable-mcp-agent", "0.1.0"),
    );
    let _server_info = client.initialize(ClientCapabilities::default()).await?;

    // Discover tools (handle pagination for completeness)
    let mut all_tools = Vec::new();
    let mut cursor = None;
    loop {
        let result = client.list_tools(cursor).await?;
        all_tools.extend(result.tools);
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(all_tools)
}
```

### ToolInfo to UnifiedTool Translation

```rust
// Source: pmcp types/tools.rs ToolInfo fields -> models.rs UnifiedTool fields
use crate::llm::models::UnifiedTool;
use crate::llm::transformers::utils::clean_tool_schema;

fn translate_mcp_tool(
    tool_info: &pmcp::types::ToolInfo,
    prefix: &str,
) -> UnifiedTool {
    UnifiedTool {
        name: format!("{prefix}__{}", tool_info.name),
        description: tool_info.description.clone().unwrap_or_default(),
        input_schema: clean_tool_schema(&tool_info.input_schema),
    }
}
```

### Error Type Design

```rust
// Source: existing LlmError pattern in error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Agent not found: {agent_name} version {version}")]
    AgentNotFound { agent_name: String, version: String },

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid JSON in field {field}: {source}")]
    InvalidJson { field: String, source: serde_json::Error },

    #[error("Unsupported LLM provider: {0}")]
    UnsupportedProvider(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("DynamoDB error: {0}")]
    DynamoDbError(String),
}

#[derive(Error, Debug)]
pub enum McpError {
    #[error("Failed to connect to MCP server {url}: {source}")]
    ConnectionFailed { url: String, source: String },

    #[error("MCP server initialization failed for {url}: {source}")]
    InitializationFailed { url: String, source: String },

    #[error("Tool discovery failed for {url}: {source}")]
    DiscoveryFailed { url: String, source: String },

    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid tool name format: {0}")]
    InvalidToolName(String),

    #[error("No MCP servers configured")]
    NoServersConfigured,
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| pmcp `HttpTransport` (HTTP only) | `StreamableHttpTransport` (HTTPS + SSE + JSON) | pmcp 2.0 | Required for HTTPS MCP servers on Lambda |
| MCP SSE-only transport | Streamable HTTP (supports both SSE and JSON response mode) | MCP spec 2024 | `enable_json_response` flag allows simpler request/response without SSE stream management |
| Custom DynamoDB attribute parsing | `aws-sdk-dynamodb` typed AttributeValue enum | Stable | Pattern match on `AttributeValue::S`, `.as_s()`, `.as_n()` etc. |

**Deprecated/outdated:**
- pmcp v1.x `HttpTransport`: Lacks TLS, uses only bare hyper `HttpConnector`. Not viable for HTTPS endpoints.
- MCP SSE-only transport mode: The streamable HTTP transport supports both SSE and JSON modes. For simple request/response patterns (list_tools, call_tool), JSON mode avoids SSE stream overhead.

## Open Questions

1. **pmcp path dependency portability**
   - What we know: pmcp v2.0.0 is at ~/Development/mcp/sdk/rust-mcp-sdk/. The agent binary uses a path dependency.
   - What's unclear: Whether the relative path (`../../sdk/rust-mcp-sdk`) will work correctly from the examples/ Cargo.toml context during SAM build. SAM copies source to a build container.
   - Recommendation: Use a relative path for development, document the CI/deployment story in Phase 5. If SAM build breaks, switch to git dependency with tag.

2. **StreamableHttpTransportConfigBuilder availability**
   - What we know: The builder pattern exists in the source code.
   - What's unclear: Whether `build()` is public or returns the right config type (it's in the same module).
   - Recommendation: Verify at implementation time. Fall back to direct struct construction if needed.

3. **DynamoDB table name discovery**
   - What we know: Table name is `AgentRegistry-{env_name}` (e.g., `AgentRegistry-prod`).
   - What's unclear: How the Lambda function knows which env. Could be environment variable.
   - Recommendation: Read table name from `AGENT_REGISTRY_TABLE` environment variable with fallback to `AgentRegistry-prod`.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | tokio::test + mockito 1.7 |
| Config file | none (standard cargo test) |
| Quick run command | `cargo test --manifest-path examples/Cargo.toml --all-targets` |
| Full suite command | `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -D warnings` |

### Phase Requirements -> Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CONF-01 | Parse DynamoDB item to AgentConfig | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib config -- test_parse_agent_config` | No - Wave 0 |
| CONF-02 | All config fields present and parsed | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib config -- test_config_fields` | No - Wave 0 |
| CONF-03 | mcp_servers JSON array parsed | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib config -- test_mcp_servers` | No - Wave 0 |
| CONF-04 | AgentConfig derives Serialize + Deserialize | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib config -- test_config_serde_round_trip` | No - Wave 0 |
| MCP-01 | MCP client connect + initialize pattern | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib mcp -- test_connect` | No - Wave 0 |
| MCP-02 | Tool discovery + merge from multiple servers | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib mcp -- test_discover_tools` | No - Wave 0 |
| MCP-03 | ToolInfo to UnifiedTool translation | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib mcp -- test_translate_tool` | No - Wave 0 |
| MCP-06 | Connection failure produces clear error | unit | `cargo test --manifest-path examples/Cargo.toml -p lambda-durable-execution-rust-examples --lib mcp -- test_connection_failure` | No - Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --manifest-path examples/Cargo.toml --all-targets`
- **Per wave merge:** `cargo test --manifest-path examples/Cargo.toml --all-targets && cargo clippy --manifest-path examples/Cargo.toml --all-targets --all-features -D warnings`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `examples/src/bin/mcp_agent/config/mod.rs` -- module declaration
- [ ] `examples/src/bin/mcp_agent/config/types.rs` -- AgentConfig, AgentParameters structs with Serialize/Deserialize
- [ ] `examples/src/bin/mcp_agent/config/loader.rs` -- DynamoDB reader with unit tests using mock DynamoDB responses
- [ ] `examples/src/bin/mcp_agent/config/error.rs` -- ConfigError enum
- [ ] `examples/src/bin/mcp_agent/mcp/mod.rs` -- module declaration
- [ ] `examples/src/bin/mcp_agent/mcp/client.rs` -- McpClientManager with unit tests using mock tool schemas
- [ ] `examples/src/bin/mcp_agent/mcp/types.rs` -- ToolsWithRouting struct
- [ ] `examples/src/bin/mcp_agent/mcp/error.rs` -- McpError enum

### Testing Strategy Notes

**Config module tests:** DynamoDB responses can be mocked by constructing `HashMap<String, AttributeValue>` directly -- no need for a running DynamoDB. Test `parse_agent_config()` with various field combinations including missing optional fields and malformed JSON strings.

**MCP module tests:** Tool translation tests construct `pmcp::types::ToolInfo` directly (it derives Default) and verify the mapping to `UnifiedTool`. Connection tests are harder to unit-test without a running MCP server. Options:
1. Test the `translate_mcp_tool()` and `extract_host_prefix()` functions in isolation (pure functions, no I/O)
2. Test `resolve_tool_call()` routing logic with mock data
3. Integration test with a mock HTTP server (mockito) that returns MCP JSON-RPC responses -- more complex, defer to Phase 5 validation

**Provider mapping tests:** Test `map_provider_config()` for each supported provider ("claude"/"anthropic", "openai") and verify the error case for unsupported providers.

## Sources

### Primary (HIGH confidence)
- `/Users/guy/projects/step-functions-agent/lambda/shared/agent_registry.py` -- DynamoDB schema, field types, JSON parsing
- `/Users/guy/projects/step-functions-agent/stacks/shared/agent_registry_stack.py` -- CDK table definition (PK: agent_name, SK: version, GSIs)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/client/mod.rs` -- pmcp Client API (initialize, list_tools, call_tool signatures)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/types/tools.rs` -- ToolInfo struct (name, description, input_schema fields)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/Cargo.toml` -- pmcp v2.0.0, feature flags (streamable-http includes rustls/ring)
- `/Users/guy/Development/mcp/sdk/rust-mcp-sdk/src/shared/streamable_http.rs` -- StreamableHttpTransport with TLS, Config, Builder
- `examples/src/bin/mcp_agent/llm/models.rs` -- ProviderConfig, UnifiedTool target types
- `examples/src/bin/mcp_agent/llm/transformers/utils.rs` -- clean_tool_schema() function
- `examples/Cargo.toml` -- current dependencies
- `examples/src/bin/mcp_agent/llm/error.rs` -- existing error pattern

### Secondary (MEDIUM confidence)
- crates.io search: `pmcp` at 1.20.0 (confirms path dep needed for v2.0.0)
- crates.io search: `aws-sdk-dynamodb` at 1.110.0 (verified current version)

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all dependencies verified from source code and crates.io
- Architecture: HIGH -- patterns directly derived from existing code (Phase 1 output) and canonical references
- Pitfalls: HIGH -- identified from direct source code analysis of pmcp transport, DynamoDB schema, and durable SDK constraints

**Research date:** 2026-03-23
**Valid until:** 2026-04-23 (stable domain -- DynamoDB SDK and pmcp local dep unlikely to change)
