mod config;
mod handler;
mod llm;
mod mcp;
mod types;

use lambda_durable_execution_rust::runtime::with_durable_execution_service;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let llm_service = llm::UnifiedLLMService::new()
        .await
        .map_err(|e| lambda_runtime::Error::from(format!("Failed to init LLM service: {e}")))?;

    let svc = with_durable_execution_service(
        move |event, ctx| {
            let llm = llm_service.clone();
            async move { handler::agent_handler(event, ctx, llm).await }
        },
        None,
    );

    lambda_runtime::run(svc).await
}
