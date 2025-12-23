use crate::types::{LambdaService, RealLambdaService};
use std::sync::Arc;

/// Configuration for durable execution.
#[derive(Debug, Clone, Default)]
pub struct DurableExecutionConfig {
    /// Custom AWS Lambda service to use.
    pub lambda_service: Option<Arc<dyn LambdaService>>,
}

impl DurableExecutionConfig {
    /// Create a new default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a custom Lambda client.
    pub fn with_lambda_client(mut self, client: aws_sdk_lambda::Client) -> Self {
        self.lambda_service = Some(Arc::new(RealLambdaService::new(Arc::new(client))));
        self
    }

    /// Set a custom Lambda service.
    pub fn with_lambda_service(mut self, service: Arc<dyn LambdaService>) -> Self {
        self.lambda_service = Some(service);
        self
    }
}
