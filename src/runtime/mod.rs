//! Runtime handler for durable Lambda functions.
//!
//! This module provides the wrapper functions that integrate durable execution
//! with the AWS Lambda runtime.
//!
//! # Overview
//!
//! The runtime module handles:
//!
//! - Parsing durable execution input from the Lambda service
//! - Setting up the execution context
//! - Running user handler functions
//! - Managing termination signals (wait, callback, retry)
//! - Checkpointing completion or failure
//! - Returning proper output format
//!
//! # Usage
//!
//! The main entry point is [`with_durable_execution_service`]:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::runtime::with_durable_execution_service;
//! use lambda_durable_execution_rust::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! async fn my_handler(
//!     _event: MyEvent,
//!     ctx: DurableContextHandle,
//! ) -> DurableResult<MyResponse> {
//!     // Your handler logic
//!     let _ = ctx.step(Some("noop"), |_| async { Ok(()) }, None).await?;
//!     Ok(MyResponse {})
//! }
//!
//! #[derive(Deserialize)]
//! struct MyEvent;
//!
//! #[derive(Serialize)]
//! struct MyResponse {}
//!
//! #[tokio::main]
//! async fn main() -> Result<(), lambda_runtime::Error> {
//!     let handler = with_durable_execution_service(my_handler, None);
//!     lambda_runtime::run(handler).await
//! }
//! ```
//!
//! # Configuration
//!
//! Use [`DurableExecutionConfig`] to customize behavior:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! use lambda_durable_execution_rust::runtime::{with_durable_execution_service, DurableExecutionConfig};
//! use std::sync::Arc;
//!
//! # async fn my_handler(_event: serde_json::Value, _ctx: DurableContextHandle) -> DurableResult<()> { Ok(()) }
//! # fn make_client() -> aws_sdk_lambda::Client {
//! #     let conf = aws_sdk_lambda::Config::builder()
//! #         .region(aws_sdk_lambda::config::Region::new("us-east-1"))
//! #         .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
//! #         .build();
//! #     aws_sdk_lambda::Client::from_conf(conf)
//! # }
//! let my_custom_client = make_client();
//! let config = DurableExecutionConfig::new().with_lambda_client(Arc::new(my_custom_client));
//!
//! let _handler = with_durable_execution_service(my_handler, Some(config));
//! ```
//!
//! # Builder Pattern
//!
//! For more ergonomic configuration, use [`durable_handler`]:
//!
//! ```rust,no_run
//! # use lambda_durable_execution_rust::prelude::*;
//! use lambda_durable_execution_rust::runtime::durable_handler;
//! use std::sync::Arc;
//!
//! # async fn my_handler(_event: serde_json::Value, _ctx: DurableContextHandle) -> DurableResult<()> { Ok(()) }
//! # fn make_client() -> aws_sdk_lambda::Client {
//! #     let conf = aws_sdk_lambda::Config::builder()
//! #         .region(aws_sdk_lambda::config::Region::new("us-east-1"))
//! #         .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
//! #         .build();
//! #     aws_sdk_lambda::Client::from_conf(conf)
//! # }
//! let custom_client = make_client();
//! let handler = durable_handler(my_handler)
//!     .with_lambda_client(Arc::new(custom_client))
//!     .build();
//!
//! // `handler` is a function over `DurableExecutionInvocationInput`; wrap it with
//! // `lambda_runtime::service_fn(...)` when running on Lambda.
//! let _ = handler;
//! ```
//!
//! # Execution Lifecycle
//!
//! 1. Lambda receives durable execution invocation
//! 2. Runtime parses input and sets up context
//! 3. Handler runs, operations are checkpointed
//! 4. If wait/callback triggered, Lambda suspends
//! 5. On completion/failure, result is checkpointed
//! 6. Response returned to Lambda service
//!
//! # Handler Signature
//!
//! Your handler function must have this signature:
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::prelude::*;
//! use serde::{de::DeserializeOwned, Serialize};
//!
//! async fn handler<E, R>(_event: E, _ctx: DurableContextHandle) -> DurableResult<R>
//! where
//!     E: DeserializeOwned + Send + 'static,
//!     R: Serialize + Send + 'static,
//! {
//!     unimplemented!()
//! }
//! ```

mod handler;

pub use handler::*;
