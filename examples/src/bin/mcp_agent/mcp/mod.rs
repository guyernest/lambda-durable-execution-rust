pub mod client;
pub mod error;
pub mod types;

// Re-exports consumed by the agent handler (Phase 3). Suppress unused warnings until then.
#[allow(unused_imports)]
pub use client::{discover_all_tools, resolve_tool_call};
#[allow(unused_imports)]
pub use error::McpError;
#[allow(unused_imports)]
pub use types::ToolsWithRouting;
