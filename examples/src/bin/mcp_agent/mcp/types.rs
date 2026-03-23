use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::llm::models::UnifiedTool;

/// Discovered tools from all MCP servers with a routing map.
///
/// This struct is the return value from `ctx.step("discover-tools")` and
/// must be serializable for checkpoint persistence. Connections themselves
/// are ephemeral and NOT stored here (per D-07).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsWithRouting {
    /// All discovered tools from all MCP servers, with prefixed names.
    pub tools: Vec<UnifiedTool>,
    /// Maps `prefixed_tool_name` to the originating `server_url` for routing
    /// tool calls back to the correct MCP server.
    pub routing: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tools_with_routing_serde_round_trip() {
        let original = ToolsWithRouting {
            tools: vec![
                UnifiedTool {
                    name: "calc__multiply".to_string(),
                    description: "Multiplies two numbers".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "a": {"type": "number"},
                            "b": {"type": "number"}
                        }
                    }),
                },
                UnifiedTool {
                    name: "wiki__search".to_string(),
                    description: "Searches the wiki".to_string(),
                    input_schema: json!({"type": "object", "properties": {}}),
                },
            ],
            routing: HashMap::from([
                (
                    "calc__multiply".to_string(),
                    "https://calc.example.com".to_string(),
                ),
                (
                    "wiki__search".to_string(),
                    "https://wiki.example.com".to_string(),
                ),
            ]),
        };

        let serialized = serde_json::to_string(&original).expect("serialize");
        let deserialized: ToolsWithRouting =
            serde_json::from_str(&serialized).expect("deserialize");

        assert_eq!(deserialized.tools.len(), 2);
        assert_eq!(deserialized.routing.len(), 2);
        assert_eq!(
            deserialized.routing.get("calc__multiply").unwrap(),
            "https://calc.example.com"
        );
        assert_eq!(deserialized.tools[0].name, "calc__multiply");
    }
}
