//! MCP server identity and shared constants.

/// The server name reported in the `initialize` handshake (`serverInfo.name`).
pub const SERVER_NAME: &str = "verbinal";

/// The server version reported in `initialize` (`serverInfo.version`).
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Max size of a single NDJSON frame (guards against a runaway document).
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
