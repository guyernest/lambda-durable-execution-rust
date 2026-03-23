use std::collections::HashMap;

use pmcp::shared::streamable_http::{StreamableHttpTransport, StreamableHttpTransportConfig};
use pmcp::types::ToolInfo;
use pmcp::{Client, ClientCapabilities, Implementation};

use crate::llm::models::UnifiedTool;
use crate::llm::transformers::utils::clean_tool_schema;
use crate::mcp::error::McpError;
use crate::mcp::types::ToolsWithRouting;

/// Discover tools from all configured MCP servers.
///
/// Connects to each server sequentially (1-3 servers, sub-second total per D-06),
/// discovers tools via `list_tools()`, translates schemas to [`UnifiedTool`] format,
/// prefixes tool names with a host-based identifier, and builds a routing map.
///
/// Fails fast if `server_urls` is empty (per MCP-06) or if any server connection
/// fails.
pub async fn discover_all_tools(server_urls: &[String]) -> Result<ToolsWithRouting, McpError> {
    if server_urls.is_empty() {
        return Err(McpError::NoServersConfigured);
    }

    let mut all_tools = Vec::new();
    let mut routing = HashMap::new();

    for url in server_urls {
        let prefix = extract_host_prefix(url)?;
        let tools = connect_and_discover(url).await?;

        for tool_info in &tools {
            let unified = translate_mcp_tool(tool_info, &prefix);
            routing.insert(unified.name.clone(), url.clone());
            all_tools.push(unified);
        }
    }

    Ok(ToolsWithRouting {
        tools: all_tools,
        routing,
    })
}

/// Connect to a single MCP server, initialize, and discover its tools.
///
/// Uses `StreamableHttpTransport` with TLS (per D-04). Connections are
/// ephemeral (per D-07) -- only the returned `ToolInfo` list persists.
async fn connect_and_discover(server_url: &str) -> Result<Vec<ToolInfo>, McpError> {
    let parsed_url =
        url::Url::parse(server_url).map_err(|_| McpError::InvalidUrl(server_url.to_string()))?;

    let config = StreamableHttpTransportConfig {
        url: parsed_url,
        extra_headers: vec![],
        auth_provider: None, // Per D-05: no auth for PoC
        session_id: None,
        enable_json_response: false,
        on_resumption_token: None,
        http_middleware_chain: None,
    };

    let transport = StreamableHttpTransport::new(config);
    let mut client =
        Client::with_info(transport, Implementation::new("durable-mcp-agent", "0.1.0"));

    client
        .initialize(ClientCapabilities::default())
        .await
        .map_err(|e| McpError::InitializationFailed {
            url: server_url.to_string(),
            reason: e.to_string(),
        })?;

    // Paginate through all tool pages (MCP-02)
    let mut all_tools = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let result = client
            .list_tools(cursor)
            .await
            .map_err(|e| McpError::DiscoveryFailed {
                url: server_url.to_string(),
                reason: e.to_string(),
            })?;

        all_tools.extend(result.tools);

        match result.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    Ok(all_tools)
}

/// Translate an MCP `ToolInfo` to a [`UnifiedTool`] with a prefixed name.
///
/// The name format is `{prefix}__{tool_name}` (per D-08). The input schema
/// is normalized via [`clean_tool_schema()`] to ensure `"type": "object"` is
/// present and empty `required` arrays are stripped.
fn translate_mcp_tool(tool_info: &ToolInfo, prefix: &str) -> UnifiedTool {
    UnifiedTool {
        name: format!("{prefix}__{}", tool_info.name),
        description: tool_info.description.clone().unwrap_or_default(),
        input_schema: clean_tool_schema(&tool_info.input_schema),
    }
}

/// Extract a short host prefix from a server URL.
///
/// Parses the URL, extracts the hostname, and takes the first segment before
/// the first dot. For example:
/// - `"https://calc-server.us-east-1.amazonaws.com/mcp"` -> `"calc-server"`
/// - `"https://wiki.example.com"` -> `"wiki"`
/// - `"https://localhost:8080"` -> `"localhost"`
fn extract_host_prefix(server_url: &str) -> Result<String, McpError> {
    let parsed =
        url::Url::parse(server_url).map_err(|_| McpError::InvalidUrl(server_url.to_string()))?;

    let host = parsed
        .host_str()
        .ok_or_else(|| McpError::InvalidUrl(server_url.to_string()))?;

    let prefix = host.split('.').next().unwrap_or(host);
    Ok(prefix.to_string())
}

/// Resolve a prefixed tool name to its originating server URL and original name.
///
/// Uses `splitn(2, "__")` (per Pitfall 4 from RESEARCH.md) to correctly handle
/// tool names that themselves contain `__`. For example:
/// - `"calc__multiply"` -> `("https://calc.example.com", "multiply")`
/// - `"calc__my__tool"` -> `("https://calc.example.com", "my__tool")`
pub fn resolve_tool_call(
    prefixed_name: &str,
    routing: &HashMap<String, String>,
) -> Result<(String, String), McpError> {
    let server_url = routing
        .get(prefixed_name)
        .ok_or_else(|| McpError::UnknownTool(prefixed_name.to_string()))?;

    let parts: Vec<&str> = prefixed_name.splitn(2, "__").collect();
    if parts.len() < 2 {
        return Err(McpError::InvalidToolName(prefixed_name.to_string()));
    }

    Ok((server_url.clone(), parts[1].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ===== extract_host_prefix tests =====

    #[test]
    fn test_extract_host_prefix_standard() {
        let prefix =
            extract_host_prefix("https://calc-server.us-east-1.amazonaws.com/mcp").unwrap();
        assert_eq!(prefix, "calc-server");
    }

    #[test]
    fn test_extract_host_prefix_simple() {
        let prefix = extract_host_prefix("https://wiki.example.com").unwrap();
        assert_eq!(prefix, "wiki");
    }

    #[test]
    fn test_extract_host_prefix_no_dots() {
        let prefix = extract_host_prefix("https://localhost:8080").unwrap();
        assert_eq!(prefix, "localhost");
    }

    #[test]
    fn test_extract_host_prefix_invalid_url() {
        let result = extract_host_prefix("not a url");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, McpError::InvalidUrl(_)));
    }

    // ===== translate_mcp_tool tests =====

    #[test]
    fn test_translate_mcp_tool_basic() {
        let tool_info = ToolInfo::new(
            "multiply",
            Some("Multiplies two numbers".to_string()),
            json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                }
            }),
        );

        let unified = translate_mcp_tool(&tool_info, "calc");
        assert_eq!(unified.name, "calc__multiply");
        assert_eq!(unified.description, "Multiplies two numbers");
        assert_eq!(unified.input_schema["type"], "object");
        assert!(unified.input_schema["properties"]["a"].is_object());
        assert!(unified.input_schema["properties"]["b"].is_object());
    }

    #[test]
    fn test_translate_mcp_tool_no_description() {
        let tool_info = ToolInfo::new("search", None, json!({"type": "object", "properties": {}}));

        let unified = translate_mcp_tool(&tool_info, "wiki");
        assert_eq!(unified.name, "wiki__search");
        assert_eq!(unified.description, "");
    }

    #[test]
    fn test_translate_mcp_tool_schema_normalized() {
        // Schema without "type": "object" should get normalized
        let tool_info = ToolInfo::new(
            "add",
            Some("Adds numbers".to_string()),
            json!({
                "properties": {
                    "x": {"type": "number"}
                }
            }),
        );

        let unified = translate_mcp_tool(&tool_info, "math");
        // clean_tool_schema ensures type: "object" is present
        assert_eq!(unified.input_schema["type"], "object");
        assert!(unified.input_schema["properties"]["x"].is_object());
    }

    // ===== resolve_tool_call tests =====

    #[test]
    fn test_resolve_tool_call_success() {
        let routing = HashMap::from([(
            "calc__multiply".to_string(),
            "https://calc.example.com".to_string(),
        )]);

        let (url, name) = resolve_tool_call("calc__multiply", &routing).unwrap();
        assert_eq!(url, "https://calc.example.com");
        assert_eq!(name, "multiply");
    }

    #[test]
    fn test_resolve_tool_call_unknown_tool() {
        let routing = HashMap::new();
        let result = resolve_tool_call("unknown__tool", &routing);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::UnknownTool(_)));
    }

    #[test]
    fn test_resolve_tool_call_no_prefix() {
        // Tool name without __ separator but present in routing should still
        // fail on the splitn check. However, since it won't be in routing,
        // it fails with UnknownTool first.
        let routing = HashMap::new();
        let result = resolve_tool_call("notool", &routing);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::UnknownTool(_)));
    }

    #[test]
    fn test_resolve_tool_call_no_prefix_in_routing() {
        // Edge case: a name without __ that IS in routing should fail on split
        let routing = HashMap::from([("notool".to_string(), "https://example.com".to_string())]);
        let result = resolve_tool_call("notool", &routing);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidToolName(_)));
    }

    #[test]
    fn test_resolve_tool_call_double_underscore_in_name() {
        // splitn(2, "__") should preserve the rest of the name
        let routing = HashMap::from([(
            "calc__my__tool".to_string(),
            "https://calc.example.com".to_string(),
        )]);

        let (url, name) = resolve_tool_call("calc__my__tool", &routing).unwrap();
        assert_eq!(url, "https://calc.example.com");
        assert_eq!(name, "my__tool");
    }

    // ===== discover_all_tools tests =====

    #[tokio::test]
    async fn test_discover_all_tools_empty_servers() {
        let result = discover_all_tools(&[]).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::NoServersConfigured));
    }
}
