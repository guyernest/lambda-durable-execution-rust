use serde_json::Value;

/// Safely extract a field from a JSON value using dot notation.
///
/// Supports both object keys and array indices (numeric path segments).
///
/// # Examples
///
/// ```rust,no_run
/// use serde_json::json;
/// // safe_extract_field(&json!({"a": {"b": 1}}), "a.b") => Some(1)
/// // safe_extract_field(&json!({"arr": [10, 20]}), "arr.1") => Some(20)
/// ```
pub fn safe_extract_field(value: &Value, path: &str) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        if let Ok(index) = part.parse::<usize>() {
            // Array index
            current = current.get(index)?;
        } else {
            // Object field
            current = current.get(part)?;
        }
    }

    Some(current.clone())
}

/// Try multiple paths and return the first successful extraction.
pub fn extract_with_fallback(value: &Value, paths: &[&str]) -> Option<Value> {
    for path in paths {
        if let Some(result) = safe_extract_field(value, path) {
            return Some(result);
        }
    }
    None
}

/// Extract a string field with fallback paths.
pub fn extract_string(value: &Value, paths: &[&str]) -> Option<String> {
    extract_with_fallback(value, paths)?
        .as_str()
        .map(|s| s.to_string())
}

/// Extract a u32 field with fallback paths and a default value.
pub fn extract_u32(value: &Value, paths: &[&str], default: u32) -> u32 {
    extract_with_fallback(value, paths)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

/// Clean and validate JSON schema for tools.
///
/// Ensures the schema is an object type with properties. Non-object inputs
/// are normalized to `{ "type": "object", "properties": {} }`.
/// Passes through standard JSON Schema fields that LLMs use for tool parameters.
pub fn clean_tool_schema(schema: &Value) -> Value {
    if let Value::Object(map) = schema {
        let mut clean = serde_json::Map::new();

        clean.insert("type".to_string(), Value::String("object".to_string()));

        // Pass through standard JSON Schema fields used by tool parameters
        for &field in &[
            "properties",
            "required",
            "additionalProperties",
            "description",
            "enum",
            "default",
            "allOf",
            "anyOf",
            "oneOf",
            "items",
        ] {
            if let Some(val) = map.get(field) {
                // Skip empty required arrays
                if field == "required" {
                    if let Value::Array(arr) = val {
                        if arr.is_empty() {
                            continue;
                        }
                    }
                }
                clean.insert(field.to_string(), val.clone());
            }
        }

        Value::Object(clean)
    } else {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_safe_extract_field_nested_path() {
        let value = json!({
            "choices": [
                {
                    "message": {
                        "content": "Hello!"
                    }
                }
            ]
        });

        let result = safe_extract_field(&value, "choices.0.message");
        assert!(result.is_some());
        let msg = result.unwrap();
        assert_eq!(msg["content"], "Hello!");
    }

    #[test]
    fn test_safe_extract_field_simple() {
        let value = json!({"name": "test"});
        let result = safe_extract_field(&value, "name");
        assert_eq!(result, Some(json!("test")));
    }

    #[test]
    fn test_safe_extract_field_missing_path() {
        let value = json!({"name": "test"});
        let result = safe_extract_field(&value, "missing.path");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_with_fallback() {
        let value = json!({"output": {"text": "found"}});
        let result = extract_with_fallback(&value, &["missing", "output.text"]);
        assert_eq!(result, Some(json!("found")));
    }

    #[test]
    fn test_extract_string() {
        let value = json!({"model": "claude-3"});
        let result = extract_string(&value, &["model"]);
        assert_eq!(result, Some("claude-3".to_string()));
    }

    #[test]
    fn test_extract_u32_with_default() {
        let value = json!({"tokens": 150});
        assert_eq!(extract_u32(&value, &["tokens"], 0), 150);
        assert_eq!(extract_u32(&value, &["missing"], 42), 42);
    }

    #[test]
    fn test_clean_tool_schema_normalizes_non_object() {
        let schema = json!("not an object");
        let cleaned = clean_tool_schema(&schema);
        assert_eq!(
            cleaned,
            json!({
                "type": "object",
                "properties": {}
            })
        );
    }

    #[test]
    fn test_clean_tool_schema_preserves_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"}
            },
            "required": ["query"]
        });
        let cleaned = clean_tool_schema(&schema);
        assert_eq!(cleaned["type"], "object");
        assert!(cleaned["properties"]["query"].is_object());
        assert_eq!(cleaned["required"], json!(["query"]));
    }

    #[test]
    fn test_clean_tool_schema_strips_empty_required() {
        let schema = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "required": []
        });
        let cleaned = clean_tool_schema(&schema);
        // Empty required array should be stripped
        assert!(cleaned.get("required").is_none());
    }

    #[test]
    fn test_clean_tool_schema_null_input() {
        let schema = Value::Null;
        let cleaned = clean_tool_schema(&schema);
        assert_eq!(
            cleaned,
            json!({
                "type": "object",
                "properties": {}
            })
        );
    }
}
