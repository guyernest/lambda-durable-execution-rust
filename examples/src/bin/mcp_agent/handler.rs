use std::collections::HashMap;
use std::sync::Arc;

use lambda_durable_execution_rust::prelude::*;
use tracing::info;

use crate::config::{load_agent_config, AgentConfig};
use crate::llm::{
    ContentBlock, FunctionCall, LLMInvocation, LLMResponse, MessageContent, UnifiedLLMService,
    UnifiedMessage, UnifiedTool,
};
use crate::mcp::{
    discover_all_tools, establish_mcp_connections, execute_tool_call, McpClientCache,
    ToolsWithRouting,
};
use crate::types::{AgentRequest, AgentResponse, IterationResult, ToolCallResult};

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
    info!(
        agent = %event.agent_name,
        version = %event.version,
        "Starting durable agent handler"
    );

    // 1. Load config from AgentRegistry via durable step (CONF-04)
    let table_name =
        std::env::var("AGENT_REGISTRY_TABLE").unwrap_or_else(|_| "AgentRegistry".to_string());
    let agent_name = event.agent_name.clone();
    let version = event.version.clone();

    let config: AgentConfig = ctx
        .step(
            Some("load-config"),
            move |_| async move {
                load_agent_config(&table_name, &agent_name, &version)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
            },
            None,
        )
        .await?;

    info!(
        provider = %config.provider_config.provider_id,
        model = %config.provider_config.model_id,
        mcp_servers = config.mcp_server_urls.len(),
        "Config loaded"
    );

    // 2. Discover tools from MCP servers via durable step (MCP-02)
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

    // 3. Establish MCP connections OUTSIDE durable steps (D-03, D-05)
    let mcp_clients: McpClientCache = if config.mcp_server_urls.is_empty() {
        Arc::new(HashMap::new())
    } else {
        establish_mcp_connections(&config.mcp_server_urls)
            .await
            .map_err(|e| DurableError::Internal(e.to_string()))?
    };

    // 4. Agent loop (LOOP-01, LOOP-04, LOOP-05, LOOP-06)
    let mut messages: Vec<UnifiedMessage> = event.messages;
    let max_iterations = config.parameters.max_iterations;

    for i in 0..max_iterations {
        info!(iteration = i, "Starting agent loop iteration");

        let llm = llm_service.clone();
        let cfg = config.clone();
        let tools = tools_with_routing.clone();
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

        // Append assistant message to history (LOOP-05)
        messages.push(iteration_result.assistant_message.clone());

        // Check if done (LOOP-07)
        if iteration_result.is_final {
            info!(iteration = i, "Agent loop completed (final response)");
            return Ok(AgentResponse {
                response: iteration_result.llm_response,
            });
        }

        // Append tool results to history (LOOP-05)
        if let Some(tool_results_msg) = &iteration_result.tool_results_message {
            messages.push(tool_results_msg.clone());
        }
    }

    // Max iterations exceeded (LOOP-06)
    Err(DurableError::Internal(format!(
        "Agent exceeded max iterations ({max_iterations}) without completing"
    )))
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

    // 4. Check if this is the final iteration
    let is_end_turn = llm_response.metadata.stop_reason.as_deref() == Some("end_turn");

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

    let routing = tools.routing.clone();
    let clients = mcp_clients.clone();

    let batch_result = ctx
        .map(
            Some("tools"),
            function_calls,
            move |call: FunctionCall, _item_ctx: DurableContextHandle, _idx: usize| {
                let r = routing.clone();
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
