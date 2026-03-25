use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aws_sdk_dynamodb::types::AttributeValue;
use lambda_durable_execution_rust::prelude::*;
use tokio::sync::OnceCell;
use tracing::info;

/// Cached DynamoDB client — initialized once per Lambda instance, reused across invocations.
/// Avoids re-loading credentials and re-creating the HTTP connection pool on every execution.
static DYNAMODB_CLIENT: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();

async fn get_dynamodb_client() -> &'static aws_sdk_dynamodb::Client {
    DYNAMODB_CLIENT
        .get_or_init(|| async {
            let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            aws_sdk_dynamodb::Client::new(&config)
        })
        .await
}

use crate::config::{load_agent_config, AgentConfig};
use crate::llm::{
    ContentBlock, FunctionCall, LLMInvocation, LLMResponse, MessageContent, UnifiedLLMService,
    UnifiedMessage, UnifiedTool,
};
use crate::mcp::{
    discover_all_tools, establish_mcp_connections, execute_tool_call, McpClientCache,
    ToolsWithRouting,
};
use crate::types::{AgentMetadata, AgentRequest, AgentResponse, IterationResult, ToolCallResult};

/// Durable agent handler implementing the full agent loop.
///
/// Loads configuration, discovers MCP tools, establishes connections, then
/// enters the agentic loop: call LLM -> execute tool calls -> append results
/// -> repeat until `end_turn` or max iterations.
///
/// Each LLM call is a durable `ctx.step()` with exponential backoff retry.
/// Tool calls are executed in parallel via `ctx.map()`. Each loop iteration
/// is wrapped in `run_in_child_context` for replay determinism.
pub async fn agent_handler(
    event: AgentRequest,
    ctx: DurableContextHandle,
    llm_service: UnifiedLLMService,
) -> DurableResult<AgentResponse> {
    // Destructure to avoid unnecessary clones
    let AgentRequest {
        agent_name,
        version,
        messages: initial_messages,
        inline_config,
        execution_id,
        executions_table,
    } = event;

    info!(
        agent = %agent_name,
        version = %version,
        "Starting durable agent handler"
    );

    // OBS-02: Start wall-clock timer for elapsed_ms tracking
    let start_time = Instant::now();

    // 1. Load config — either from inline payload or DynamoDB registry
    let config: AgentConfig = if let Some(inline) = inline_config {
        info!("Using inline config (pmcp-run mode)");
        inline
            .to_agent_config(&agent_name, &version)
            .map_err(|e| DurableError::Internal(e.to_string()))?
    } else {
        // Registry mode: load from DynamoDB via durable step
        let table_name =
            std::env::var("AGENT_REGISTRY_TABLE").unwrap_or_else(|_| "AgentRegistry".to_string());
        let an = agent_name.clone();
        let ver = version.clone();

        ctx.step(
            Some("load-config"),
            move |_| async move {
                load_agent_config(&table_name, &an, &ver)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            None,
        )
        .await?
    };

    info!(
        provider = %config.provider_config.provider_id,
        model = %config.provider_config.model_id,
        mcp_servers = config.mcp_server_urls.len(),
        "Config loaded"
    );

    // 2. Discover tools from MCP servers via durable step
    let tools_with_routing: ToolsWithRouting = if config.mcp_server_urls.is_empty() {
        ToolsWithRouting {
            tools: vec![],
            routing: HashMap::new(),
        }
    } else {
        let urls = config.mcp_server_urls.clone();
        ctx.step(
            Some("discover-tools"),
            move |_| async move {
                discover_all_tools(&urls)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            None,
        )
        .await?
    };

    info!(tools = tools_with_routing.tools.len(), "Tools discovered");

    // 3. Establish MCP connections OUTSIDE durable steps
    let mcp_clients: McpClientCache = if config.mcp_server_urls.is_empty() {
        Arc::new(HashMap::new())
    } else {
        establish_mcp_connections(&config.mcp_server_urls)
            .await
            .map_err(|e| DurableError::Internal(e.to_string()))?
    };

    // 4. Agent loop
    let mut messages: Vec<UnifiedMessage> = initial_messages;
    let max_iterations = config.parameters.max_iterations;
    let config = Arc::new(config);
    let tools_with_routing = Arc::new(tools_with_routing);

    // OBS-01: Initialize token and tool accumulation
    let mut total_input_tokens: u32 = 0;
    let mut total_output_tokens: u32 = 0;
    let mut tools_called: Vec<String> = Vec::new();

    for i in 0..max_iterations {
        info!(iteration = i, "Starting agent loop iteration");

        let llm = llm_service.clone();
        let cfg = Arc::clone(&config);
        let tools = Arc::clone(&tools_with_routing);
        let msgs = messages.clone();
        let clients = mcp_clients.clone();

        let iteration_result: IterationResult = ctx
            .run_in_child_context(
                Some(&format!("iteration-{i}")),
                move |child_ctx| async move {
                    execute_iteration(child_ctx, &llm, &cfg, &tools, &msgs, &clients, i).await
                },
                None,
            )
            .await?;

        // Destructure to avoid clones (LOOP-05)
        let IterationResult {
            llm_response,
            assistant_message,
            tool_results_message,
            is_final,
        } = iteration_result;

        // OBS-01: Accumulate token usage from this iteration's LLM response
        if let Some(ref tokens) = llm_response.metadata.tokens_used {
            total_input_tokens += tokens.input_tokens;
            total_output_tokens += tokens.output_tokens;
        }

        // OBS-02: Collect tool names from this iteration
        if let Some(ref calls) = llm_response.function_calls {
            for call in calls {
                tools_called.push(call.name.clone());
            }
        }

        // OBS-03: Structured end-of-iteration log
        info!(
            iteration = i,
            input_tokens = llm_response
                .metadata
                .tokens_used
                .as_ref()
                .map(|t| t.input_tokens)
                .unwrap_or(0),
            output_tokens = llm_response
                .metadata
                .tokens_used
                .as_ref()
                .map(|t| t.output_tokens)
                .unwrap_or(0),
            total_input_tokens,
            total_output_tokens,
            tool_count = llm_response
                .function_calls
                .as_ref()
                .map(|c| c.len())
                .unwrap_or(0),
            is_final,
            "Iteration complete"
        );

        messages.push(assistant_message);

        // Check if done (LOOP-07)
        if is_final {
            let elapsed = start_time.elapsed();
            let elapsed_ms = elapsed.as_millis() as u64;
            info!(
                iteration = i,
                total_input_tokens,
                total_output_tokens,
                tools_called = ?tools_called,
                elapsed_ms,
                "Agent loop completed (final response)"
            );

            // Extract final text for execution record
            let final_text = extract_final_text(&llm_response);

            update_execution_status(&ExecutionUpdate {
                execution_id: &execution_id,
                executions_table: &executions_table,
                status: "completed",
                output: Some(&final_text),
                error_message: None,
                iterations: i + 1,
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                tools_called: &tools_called,
                elapsed_ms,
            })
            .await;

            return Ok(AgentResponse {
                response: llm_response,
                agent_metadata: Some(AgentMetadata {
                    iterations: i + 1,
                    total_input_tokens,
                    total_output_tokens,
                    tools_called,
                    elapsed_ms,
                }),
            });
        }

        // Append tool results to history
        if let Some(tool_msg) = tool_results_message {
            messages.push(tool_msg);
        }
    }

    // Max iterations exceeded
    let elapsed = start_time.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let error_msg = format!(
        "Agent exceeded max iterations ({max_iterations}) without completing"
    );
    info!(
        max_iterations,
        total_input_tokens,
        total_output_tokens,
        tools_called = ?tools_called,
        elapsed_ms,
        "Agent exceeded max iterations"
    );

    update_execution_status(&ExecutionUpdate {
        execution_id: &execution_id,
        executions_table: &executions_table,
        status: "failed",
        output: None,
        error_message: Some(&error_msg),
        iterations: max_iterations,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        tools_called: &tools_called,
        elapsed_ms,
    })
    .await;

    Err(DurableError::Internal(error_msg))
}

/// Execute a single agent loop iteration within a child context.
///
/// Calls the LLM with retry, checks for end_turn, executes tool calls in
/// parallel via `ctx.map()`, and assembles the iteration result.
async fn execute_iteration(
    ctx: DurableContextHandle,
    llm: &UnifiedLLMService,
    config: &AgentConfig,
    tools: &ToolsWithRouting,
    messages: &[UnifiedMessage],
    mcp_clients: &McpClientCache,
    iteration: u32,
) -> DurableResult<IterationResult> {
    // 1. Build the LLM invocation
    let invocation = build_llm_invocation(config, messages, &tools.tools);

    // 2. Call LLM with exponential backoff retry (LOOP-02, D-09)
    let llm_clone = llm.clone();
    let retry = ExponentialBackoff::builder()
        .max_attempts(3)
        .initial_delay(Duration::seconds(2))
        .max_delay(Duration::seconds(30))
        .backoff_rate(2.0)
        .build();
    let step_config = StepConfig::<LLMResponse>::new().with_retry_strategy(Arc::new(retry));

    let llm_response: LLMResponse = ctx
        .step(
            Some("llm-call"),
            move |_| async move {
                llm_clone
                    .process(invocation)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            Some(step_config),
        )
        .await?;

    // 3. Build assistant message from LLM response
    let assistant_message = llm_response_to_assistant_message(&llm_response);

    // 4. Check if this is the final iteration (handle both Anthropic and OpenAI stop reasons)
    let is_end_turn = matches!(
        llm_response.metadata.stop_reason.as_deref(),
        Some("end_turn" | "stop")
    );

    let function_calls = llm_response.function_calls.clone().unwrap_or_default();
    let has_tool_calls = !function_calls.is_empty();

    // If end_turn OR no tool calls, this is the final iteration
    if is_end_turn || !has_tool_calls {
        info!(
            iteration,
            stop_reason = ?llm_response.metadata.stop_reason,
            "Iteration complete (final)"
        );
        return Ok(IterationResult {
            llm_response,
            assistant_message,
            tool_results_message: None,
            is_final: true,
        });
    }

    // 5. Execute tool calls in parallel via ctx.map() (LOOP-03, MCP-04, MCP-05)
    info!(
        iteration,
        tool_calls = function_calls.len(),
        "Executing tool calls"
    );

    let routing = Arc::new(tools.routing.clone());
    let clients = mcp_clients.clone();

    let batch_result = ctx
        .map(
            Some("tools"),
            function_calls,
            move |call: FunctionCall, _item_ctx: DurableContextHandle, _idx: usize| {
                let r = Arc::clone(&routing);
                let c = clients.clone();
                async move {
                    execute_tool_call(&call, &r, &c)
                        .await
                        .map_err(|e| DurableError::Internal(e.to_string()))
                }
            },
            None,
        )
        .await?;

    let tool_results: Vec<ToolCallResult> = batch_result.values();

    // 6. Build tool results message
    let tool_results_message = build_tool_results_message(tool_results);

    Ok(IterationResult {
        llm_response,
        assistant_message,
        tool_results_message: Some(tool_results_message),
        is_final: false,
    })
}

/// Build an LLM invocation from agent config, message history, and tools.
fn build_llm_invocation(
    config: &AgentConfig,
    messages: &[UnifiedMessage],
    tools: &[UnifiedTool],
) -> LLMInvocation {
    // Prepend system prompt as the first message
    let mut all_messages = vec![UnifiedMessage {
        role: "system".to_string(),
        content: MessageContent::Text {
            content: config.system_prompt.clone(),
        },
    }];
    all_messages.extend_from_slice(messages);

    LLMInvocation {
        provider_config: config.provider_config.clone(),
        messages: all_messages,
        tools: if tools.is_empty() {
            None
        } else {
            Some(tools.to_vec())
        },
        temperature: Some(config.parameters.temperature),
        max_tokens: Some(config.parameters.max_tokens as i32),
        top_p: None,
        stream: None,
    }
}

/// Convert an LLM response to an assistant message for the conversation history.
fn llm_response_to_assistant_message(response: &LLMResponse) -> UnifiedMessage {
    UnifiedMessage {
        role: "assistant".to_string(),
        content: MessageContent::Blocks {
            content: response.message.content.clone(),
        },
    }
}

/// Build a user message containing tool results for the conversation history.
///
/// Each `ToolCallResult` becomes a `ContentBlock::ToolResult`. MCP errors
/// (`is_error: true`) are passed through so the LLM can decide recovery
/// (per D-12, MCP-05).
fn build_tool_results_message(results: Vec<ToolCallResult>) -> UnifiedMessage {
    let blocks: Vec<ContentBlock> = results
        .into_iter()
        .map(|r| ContentBlock::ToolResult {
            tool_use_id: r.tool_use_id,
            content: r.content,
            is_error: if r.is_error { Some(true) } else { None },
        })
        .collect();

    UnifiedMessage {
        role: "user".to_string(),
        content: MessageContent::Blocks { content: blocks },
    }
}

/// Extract final text from the LLM response for the execution record.
fn extract_final_text(response: &LLMResponse) -> String {
    response
        .message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Execution status update to write back to the AgentExecution DynamoDB table.
struct ExecutionUpdate<'a> {
    execution_id: &'a Option<String>,
    executions_table: &'a Option<String>,
    status: &'a str,
    output: Option<&'a str>,
    error_message: Option<&'a str>,
    iterations: u32,
    input_tokens: u32,
    output_tokens: u32,
    tools_called: &'a [String],
    elapsed_ms: u64,
}

/// Write execution status back to the AgentExecution DynamoDB table.
/// No-op if execution_id or executions_table is None.
///
/// Intentionally NOT a durable step — the update is idempotent (SET-only)
/// so replaying it is safe, and checkpointing a `()` return adds overhead
/// with no benefit.
async fn update_execution_status(update: &ExecutionUpdate<'_>) {
    let (Some(exec_id), Some(table)) = (update.execution_id, update.executions_table) else {
        return;
    };

    let client = get_dynamodb_client().await;

    let mut update_expr = String::from(
        "SET #status = :status, iterations = :iterations, \
         inputTokens = :input_tokens, outputTokens = :output_tokens, \
         totalTokens = :total_tokens, toolsCalled = :tools_called, \
         durationMs = :duration_ms, updatedAt = :updated_at, \
         completedAt = :completed_at"
    );
    let mut expr_values = HashMap::new();
    let mut expr_names = HashMap::new();

    expr_names.insert("#status".to_string(), "status".to_string());

    expr_values.insert(":status".to_string(), AttributeValue::S(update.status.to_string()));
    expr_values.insert(":iterations".to_string(), AttributeValue::N(update.iterations.to_string()));
    expr_values.insert(":input_tokens".to_string(), AttributeValue::N(update.input_tokens.to_string()));
    expr_values.insert(":output_tokens".to_string(), AttributeValue::N(update.output_tokens.to_string()));
    expr_values.insert(":total_tokens".to_string(), AttributeValue::N((update.input_tokens + update.output_tokens).to_string()));
    expr_values.insert(
        ":tools_called".to_string(),
        AttributeValue::L(update.tools_called.iter().map(|t| AttributeValue::S(t.clone())).collect()),
    );
    expr_values.insert(":duration_ms".to_string(), AttributeValue::N(update.elapsed_ms.to_string()));
    let now = chrono::Utc::now().to_rfc3339();
    expr_values.insert(":updated_at".to_string(), AttributeValue::S(now.clone()));
    expr_values.insert(":completed_at".to_string(), AttributeValue::S(now));

    if let Some(out) = update.output {
        update_expr.push_str(", output = :output");
        expr_values.insert(":output".to_string(), AttributeValue::S(out.to_string()));
    }
    if let Some(err) = update.error_message {
        update_expr.push_str(", errorMessage = :error_msg");
        expr_values.insert(":error_msg".to_string(), AttributeValue::S(err.to_string()));
    }

    if let Err(e) = client
        .update_item()
        .table_name(table)
        .key("id", AttributeValue::S(exec_id.clone()))
        .update_expression(update_expr)
        .set_expression_attribute_names(Some(expr_names))
        .set_expression_attribute_values(Some(expr_values))
        .send()
        .await
    {
        tracing::warn!(
            execution_id = %exec_id,
            table = %table,
            error = %e,
            error_debug = ?e,
            "Failed to update execution status — non-fatal"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::AgentParameters;
    use crate::llm::models::{
        AssistantMessage, ContentBlock, FunctionCall, ProviderConfig, ResponseMetadata, TokenUsage,
    };
    use serde_json::json;

    fn make_test_config() -> AgentConfig {
        AgentConfig {
            agent_name: "test-agent".to_string(),
            version: "v1".to_string(),
            system_prompt: "You are a helpful assistant.".to_string(),
            provider_config: ProviderConfig {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet-4-20250514".to_string(),
                endpoint: "https://api.anthropic.com/v1/messages".to_string(),
                auth_header_name: "x-api-key".to_string(),
                auth_header_prefix: None,
                secret_path: "test/secret".to_string(),
                secret_key_name: "api_key".to_string(),
                request_transformer: "anthropic_v1".to_string(),
                response_transformer: "anthropic_v1".to_string(),
                timeout: 120,
                custom_headers: None,
            },
            mcp_server_urls: vec![],
            parameters: AgentParameters {
                max_iterations: 10,
                temperature: 0.7,
                max_tokens: 4096,
                timeout_seconds: 120,
            },
        }
    }

    fn make_test_llm_response(stop_reason: &str, with_tool_calls: bool) -> LLMResponse {
        let mut content = vec![ContentBlock::Text {
            text: "Hello!".to_string(),
        }];
        let function_calls = if with_tool_calls {
            content.push(ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "calc__multiply".to_string(),
                input: json!({"a": 2, "b": 3}),
            });
            Some(vec![FunctionCall {
                id: "tu_1".to_string(),
                name: "calc__multiply".to_string(),
                input: json!({"a": 2, "b": 3}),
            }])
        } else {
            None
        };

        LLMResponse {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: None,
            },
            function_calls,
            metadata: ResponseMetadata {
                model_id: "claude-sonnet-4-20250514".to_string(),
                provider_id: "anthropic".to_string(),
                latency_ms: 500,
                tokens_used: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    total_tokens: 150,
                }),
                stop_reason: Some(stop_reason.to_string()),
            },
        }
    }

    #[test]
    fn test_build_llm_invocation_prepends_system_prompt() {
        let config = make_test_config();
        let messages = vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "What is 2+2?".to_string(),
            },
        }];
        let tools = vec![UnifiedTool {
            name: "calc__add".to_string(),
            description: "Adds numbers".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];

        let invocation = build_llm_invocation(&config, &messages, &tools);

        // First message should be the system prompt
        assert_eq!(invocation.messages[0].role, "system");
        match &invocation.messages[0].content {
            MessageContent::Text { content } => {
                assert_eq!(content, "You are a helpful assistant.");
            }
            _ => panic!("Expected Text content for system message"),
        }

        // Second message should be the user message
        assert_eq!(invocation.messages[1].role, "user");

        // Total messages: system + 1 user = 2
        assert_eq!(invocation.messages.len(), 2);

        // Tools should be Some when non-empty
        assert!(invocation.tools.is_some());
        assert_eq!(invocation.tools.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_build_llm_invocation_empty_tools() {
        let config = make_test_config();
        let messages = vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        }];

        let invocation = build_llm_invocation(&config, &messages, &[]);

        assert!(invocation.tools.is_none());
    }

    #[test]
    fn test_build_llm_invocation_passes_temperature_and_max_tokens() {
        let mut config = make_test_config();
        config.parameters.temperature = 0.3;
        config.parameters.max_tokens = 2048;

        let messages = vec![UnifiedMessage {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        }];

        let invocation = build_llm_invocation(&config, &messages, &[]);

        assert_eq!(invocation.temperature, Some(0.3));
        assert_eq!(invocation.max_tokens, Some(2048));
    }

    #[test]
    fn test_llm_response_to_assistant_message() {
        let response = make_test_llm_response("tool_use", true);
        let msg = llm_response_to_assistant_message(&response);

        assert_eq!(msg.role, "assistant");
        match &msg.content {
            MessageContent::Blocks { content } => {
                assert_eq!(content.len(), 2);
                // First block should be Text
                match &content[0] {
                    ContentBlock::Text { text } => assert_eq!(text, "Hello!"),
                    other => panic!("Expected Text block, got: {other:?}"),
                }
                // Second block should be ToolUse
                match &content[1] {
                    ContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "tu_1");
                        assert_eq!(name, "calc__multiply");
                        assert_eq!(input, &json!({"a": 2, "b": 3}));
                    }
                    other => panic!("Expected ToolUse block, got: {other:?}"),
                }
            }
            other => panic!("Expected Blocks content, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_tool_results_message_success() {
        let results = vec![ToolCallResult {
            tool_use_id: "tu_1".to_string(),
            content: "result".to_string(),
            is_error: false,
        }];

        let msg = build_tool_results_message(results);

        assert_eq!(msg.role, "user");
        match &msg.content {
            MessageContent::Blocks { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        assert_eq!(tool_use_id, "tu_1");
                        assert_eq!(content, "result");
                        // is_error should be None (not Some(false)) for JSON compat
                        assert!(
                            is_error.is_none(),
                            "is_error should be None for success, not Some(false)"
                        );
                    }
                    other => panic!("Expected ToolResult block, got: {other:?}"),
                }
            }
            other => panic!("Expected Blocks content, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_tool_results_message_with_error() {
        let results = vec![ToolCallResult {
            tool_use_id: "tu_err".to_string(),
            content: "Tool execution failed".to_string(),
            is_error: true,
        }];

        let msg = build_tool_results_message(results);

        assert_eq!(msg.role, "user");
        match &msg.content {
            MessageContent::Blocks { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentBlock::ToolResult { is_error, .. } => {
                        // MCP-05: is_error: true produces Some(true)
                        assert_eq!(*is_error, Some(true));
                    }
                    other => panic!("Expected ToolResult block, got: {other:?}"),
                }
            }
            other => panic!("Expected Blocks content, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_tool_results_message_multiple() {
        let results = vec![
            ToolCallResult {
                tool_use_id: "tu_1".to_string(),
                content: "result 1".to_string(),
                is_error: false,
            },
            ToolCallResult {
                tool_use_id: "tu_2".to_string(),
                content: "result 2".to_string(),
                is_error: false,
            },
            ToolCallResult {
                tool_use_id: "tu_3".to_string(),
                content: "error result".to_string(),
                is_error: true,
            },
        ];

        let msg = build_tool_results_message(results);

        assert_eq!(msg.role, "user");
        match &msg.content {
            MessageContent::Blocks { content } => {
                assert_eq!(content.len(), 3);
                // Verify each block is a ToolResult
                for block in content {
                    assert!(
                        matches!(block, ContentBlock::ToolResult { .. }),
                        "Expected ToolResult block"
                    );
                }
            }
            other => panic!("Expected Blocks content, got: {other:?}"),
        }
    }

    #[test]
    fn test_token_accumulation_from_llm_response() {
        // Simulate the accumulation logic from agent_handler
        let responses = vec![
            make_test_llm_response("tool_use", true), // 100 input, 50 output
            make_test_llm_response("tool_use", true), // 100 input, 50 output
            make_test_llm_response("end_turn", false), // 100 input, 50 output
        ];

        let mut total_input_tokens: u32 = 0;
        let mut total_output_tokens: u32 = 0;

        for resp in &responses {
            if let Some(ref tokens) = resp.metadata.tokens_used {
                total_input_tokens += tokens.input_tokens;
                total_output_tokens += tokens.output_tokens;
            }
        }

        assert_eq!(total_input_tokens, 300, "3 responses x 100 input tokens");
        assert_eq!(total_output_tokens, 150, "3 responses x 50 output tokens");
    }

    #[test]
    fn test_token_accumulation_handles_missing_tokens() {
        // Verify accumulation works when tokens_used is None
        let mut response = make_test_llm_response("end_turn", false);
        response.metadata.tokens_used = None;

        let mut total_input_tokens: u32 = 0;
        let mut total_output_tokens: u32 = 0;

        // First response has tokens (100/50)
        let resp_with_tokens = make_test_llm_response("tool_use", true);
        if let Some(ref tokens) = resp_with_tokens.metadata.tokens_used {
            total_input_tokens += tokens.input_tokens;
            total_output_tokens += tokens.output_tokens;
        }

        // Second response has no tokens
        if let Some(ref tokens) = response.metadata.tokens_used {
            total_input_tokens += tokens.input_tokens;
            total_output_tokens += tokens.output_tokens;
        }

        assert_eq!(total_input_tokens, 100, "Only first response had tokens");
        assert_eq!(total_output_tokens, 50, "Only first response had tokens");
    }

    #[test]
    fn test_tools_called_collection() {
        // Simulate the tool name collection logic from agent_handler
        let responses = vec![
            make_test_llm_response("tool_use", true), // has calc__multiply
            make_test_llm_response("tool_use", true), // has calc__multiply
            make_test_llm_response("end_turn", false), // no tool calls
        ];

        let mut tools_called: Vec<String> = Vec::new();

        for resp in &responses {
            if let Some(ref calls) = resp.function_calls {
                for call in calls {
                    tools_called.push(call.name.clone());
                }
            }
        }

        assert_eq!(tools_called.len(), 2);
        assert_eq!(tools_called[0], "calc__multiply");
        assert_eq!(tools_called[1], "calc__multiply");
    }

    #[test]
    fn test_tools_called_preserves_order_and_duplicates() {
        // Verify tools are NOT deduplicated and order is preserved
        let mut resp1 = make_test_llm_response("tool_use", false);
        resp1.function_calls = Some(vec![
            FunctionCall {
                id: "tu_1".to_string(),
                name: "search__query".to_string(),
                input: json!({"q": "test"}),
            },
            FunctionCall {
                id: "tu_2".to_string(),
                name: "calc__add".to_string(),
                input: json!({"a": 1, "b": 2}),
            },
        ]);

        let mut resp2 = make_test_llm_response("tool_use", false);
        resp2.function_calls = Some(vec![FunctionCall {
            id: "tu_3".to_string(),
            name: "search__query".to_string(),
            input: json!({"q": "another"}),
        }]);

        let mut tools_called: Vec<String> = Vec::new();
        for resp in &[resp1, resp2] {
            if let Some(ref calls) = resp.function_calls {
                for call in calls {
                    tools_called.push(call.name.clone());
                }
            }
        }

        assert_eq!(tools_called.len(), 3);
        assert_eq!(tools_called[0], "search__query");
        assert_eq!(tools_called[1], "calc__add");
        assert_eq!(tools_called[2], "search__query"); // duplicate preserved
    }
}
