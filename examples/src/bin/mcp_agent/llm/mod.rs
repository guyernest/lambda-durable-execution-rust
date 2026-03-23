pub mod error;
pub mod models;
pub mod transformers;

// Re-exports used by service (plan 02) and agent handler (plan 03).
#[allow(unused_imports)]
pub use error::LlmError;
#[allow(unused_imports)]
pub use models::{
    AssistantMessage, ContentBlock, FunctionCall, LLMInvocation, LLMResponse, ProviderConfig,
    ResponseMetadata, TokenUsage, UnifiedMessage, UnifiedTool,
};
