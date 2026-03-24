// Allow dead code until the handler is fully wired.
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod llm;
#[allow(dead_code)]
mod mcp;
#[allow(dead_code)]
mod types;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    // Handler will be wired in Phase 3
    Ok(())
}
