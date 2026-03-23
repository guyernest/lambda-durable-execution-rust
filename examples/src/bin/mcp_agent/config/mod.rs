pub mod error;
pub mod loader;
pub mod types;

// Re-exports consumed by the agent handler (Phase 3). Suppress unused warnings until then.
#[allow(unused_imports)]
pub use error::ConfigError;
#[allow(unused_imports)]
pub use loader::{load_agent_config, map_provider_config, parse_agent_config};
#[allow(unused_imports)]
pub use types::{AgentConfig, AgentParameters};
