//! MCP plumbing for tool-bench: a stdio connection to a server process, a streamable-HTTP
//! front end, the stdio→HTTP bridge, clients for both transports, and tool-list snapshots.

pub mod bridge;
pub mod client;
pub mod front;
pub mod http_client;
pub mod launch;
pub mod replica;
pub mod report;
pub mod snapshot;
pub mod stdio;

/// MCP protocol revision this crate speaks as a client.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
