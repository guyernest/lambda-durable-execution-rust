use super::*;
use crate::error::BoxError;
use crate::mock::MockLambdaService;
use crate::retry::NoRetry;
use crate::types::{BatchResult, Duration, JsonSerdes, Serdes};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Clone, Copy, Default)]
struct NoopSerdes;

#[async_trait]
impl<T: Send + Sync> Serdes<T> for NoopSerdes {
    async fn serialize(
        &self,
        _value: Option<&T>,
        _context: crate::types::SerdesContext,
    ) -> Result<Option<String>, BoxError> {
        Ok(None)
    }

    async fn deserialize(
        &self,
        _data: Option<&str>,
        _context: crate::types::SerdesContext,
    ) -> Result<Option<T>, BoxError> {
        Ok(None)
    }
}

#[test]
fn test_step_config_builder() {
    let config: StepConfig<String> =
        StepConfig::new().with_semantics(StepSemantics::AtLeastOncePerRetry);

    assert_eq!(config.semantics, StepSemantics::AtLeastOncePerRetry);
    assert!(config.retry_strategy.is_none());
}

#[test]
fn test_callback_config_builder() {
    let config: CallbackConfig<String> = CallbackConfig::new()
        .with_timeout(Duration::minutes(5))
        .with_heartbeat_timeout(Duration::seconds(30));

    assert_eq!(config.timeout.unwrap().to_seconds(), 300);
    assert_eq!(config.heartbeat_timeout.unwrap().to_seconds(), 30);
}

#[test]
fn test_invoke_config_builder_and_clone() {
    let config: InvokeConfig<i32, i32> = InvokeConfig::new()
        .with_payload_serdes(Arc::new(JsonSerdes))
        .with_result_serdes(Arc::new(JsonSerdes))
        .with_tenant_id("tenant-1");

    assert!(config.payload_serdes.is_some());
    assert!(config.result_serdes.is_some());
    assert_eq!(config.tenant_id.as_deref(), Some("tenant-1"));

    let cloned = config.clone();
    assert!(cloned.payload_serdes.is_some());
    assert!(cloned.result_serdes.is_some());
    assert_eq!(cloned.tenant_id.as_deref(), Some("tenant-1"));
}

#[test]
fn test_child_context_config_builder() {
    let config: ChildContextConfig<i32> = ChildContextConfig::new()
        .with_sub_type("child")
        .with_serdes(Arc::new(JsonSerdes));

    assert_eq!(config.sub_type.as_deref(), Some("child"));
    assert!(config.serdes.is_some());
}

#[test]
fn test_completion_config_builder() {
    let config = CompletionConfig::new()
        .with_min_successful(2)
        .with_tolerated_failures(1)
        .with_tolerated_failure_percentage(25.0);

    assert_eq!(config.min_successful, Some(2));
    assert_eq!(config.tolerated_failure_count, Some(1));
    assert_eq!(config.tolerated_failure_percentage, Some(25.0));
}

#[test]
fn test_parallel_config_builder() {
    let batch_serdes: Arc<dyn Serdes<BatchResult<i32>>> = Arc::new(NoopSerdes);
    let item_serdes: Arc<dyn Serdes<i32>> = Arc::new(NoopSerdes);

    let config: ParallelConfig<i32> = ParallelConfig::new()
        .with_max_concurrency(5)
        .with_completion_config(
            CompletionConfig::new()
                .with_min_successful(3)
                .with_tolerated_failures(2),
        )
        .with_serdes(batch_serdes.clone())
        .with_item_serdes(item_serdes.clone());

    assert_eq!(config.max_concurrency, Some(5));
    assert_eq!(config.completion_config.min_successful, Some(3));
    assert_eq!(config.completion_config.tolerated_failure_count, Some(2));
    assert!(config.serdes.is_some());
    assert!(config.item_serdes.is_some());
}

#[test]
fn test_map_config_builder_and_clone() {
    let batch_serdes: Arc<dyn Serdes<BatchResult<i32>>> = Arc::new(NoopSerdes);
    let item_serdes: Arc<dyn Serdes<i32>> = Arc::new(NoopSerdes);
    let item_namer: Arc<ItemNamer<i32>> = Arc::new(|item, idx| format!("{item}-{idx}"));

    let config: MapConfig<i32, i32> = MapConfig::new()
        .with_max_concurrency(4)
        .with_item_namer(item_namer)
        .with_serdes(batch_serdes)
        .with_item_serdes(item_serdes)
        .with_completion_config(CompletionConfig::new().with_min_successful(1));

    assert_eq!(config.max_concurrency, Some(4));
    assert!(config.item_namer.is_some());
    assert!(config.serdes.is_some());
    assert!(config.item_serdes.is_some());
    assert_eq!(config.completion_config.min_successful, Some(1));

    let cloned = config.clone();
    assert_eq!(cloned.max_concurrency, Some(4));
    assert!(cloned.item_namer.is_some());
    assert!(cloned.serdes.is_some());
    assert!(cloned.item_serdes.is_some());
    assert_eq!(cloned.completion_config.min_successful, Some(1));
}

#[test]
fn test_named_parallel_branch_naming() {
    let branch = NamedParallelBranch::new(|| ());
    assert!(branch.name.is_none());

    let named = NamedParallelBranch::new(|| ()).with_name("branch-1");
    assert_eq!(named.name.as_deref(), Some("branch-1"));
}

#[test]
fn test_wait_condition_config_builder() {
    let wait_strategy = Arc::new(|_state: &i32, _attempt: u32| WaitConditionDecision::Stop);
    let config = WaitConditionConfig::new(0, wait_strategy)
        .with_max_attempts(3)
        .with_serdes(Arc::new(JsonSerdes));

    assert_eq!(config.initial_state, 0);
    assert_eq!(config.max_attempts, Some(3));
    assert!(config.serdes.is_some());
}

#[test]
fn test_step_config_with_retry_strategy() {
    let config: StepConfig<String> =
        StepConfig::new().with_retry_strategy(Arc::new(NoRetry::new()));

    assert!(config.retry_strategy.is_some());
}

#[test]
fn test_durable_execution_config_with_lambda_service() {
    let config =
        DurableExecutionConfig::new().with_lambda_service(Arc::new(MockLambdaService::new()));

    assert!(config.lambda_service.is_some());
}

#[test]
fn test_durable_execution_config_with_lambda_client() {
    let sdk_config = aws_sdk_lambda::Config::builder()
        .region(aws_sdk_lambda::config::Region::new("us-east-1"))
        .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
        .build();
    let client = aws_sdk_lambda::Client::from_conf(sdk_config);

    let config = DurableExecutionConfig::new().with_lambda_client(client);

    assert!(config.lambda_service.is_some());
}
