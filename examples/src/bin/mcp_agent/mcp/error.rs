use thiserror::Error;

/// Errors from MCP server connections and tool discovery.
#[derive(Error, Debug)]
pub enum McpError {
    /// Failed to establish a connection to the MCP server.
    #[error("Failed to connect to MCP server {url}: {reason}")]
    ConnectionFailed {
        /// The server URL that failed.
        url: String,
        /// The underlying error description.
        reason: String,
    },

    /// MCP server initialization handshake failed.
    #[error("MCP server initialization failed for {url}: {reason}")]
    InitializationFailed {
        /// The server URL that failed initialization.
        url: String,
        /// The underlying error description.
        reason: String,
    },

    /// Tool discovery (list_tools) failed.
    #[error("Tool discovery failed for {url}: {reason}")]
    DiscoveryFailed {
        /// The server URL that failed discovery.
        url: String,
        /// The underlying error description.
        reason: String,
    },

    /// Tool name not found in the routing map.
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    /// Tool name missing the required host prefix separator `__`.
    #[error("Invalid tool name format (missing prefix): {0}")]
    InvalidToolName(String),

    /// No MCP servers were configured for the agent.
    #[error("No MCP servers configured")]
    NoServersConfigured,

    /// The provided server URL could not be parsed.
    #[error("Invalid server URL: {0}")]
    InvalidUrl(String),
}
