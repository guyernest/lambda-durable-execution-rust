// Allow dead code until the handler is wired in Phase 3.
#[allow(dead_code)]
mod llm;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    // Handler will be wired in Phase 3
    Ok(())
}
