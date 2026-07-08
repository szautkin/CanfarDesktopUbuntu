//! Model Context Protocol server.
//!
//! Lets an AI agent (e.g. Claude Desktop or Claude Code CLI) drive Verbinal over
//! MCP. Linux design (a deliberate equivalent of the Windows named-pipe + sidecar):
//! the running app hosts a per-user UNIX-domain socket under `$XDG_RUNTIME_DIR`
//! (owner-only), and `verbinal mcp` runs a thin stdio↔socket bridge that the MCP
//! client launches.
//!
//! Layers: [`jsonrpc`] (wire) · [`framing`] (NDJSON) · [`server`] (per-connection
//! dispatch) · [`listener`] (accept loop) · [`bridge`] (stdio relay) · [`tools`]
//! (the tool abstractions + proposal pipeline).

pub mod budget;
pub mod agent_events;
pub mod audit;
pub mod client_approval;
pub mod diagnostics;
pub mod config;
pub mod constants;
pub mod jsonrpc;
pub mod preview;
pub mod selftest;
pub mod tools;
pub mod view_state;

// Transport + server layers.
pub mod bridge;
pub mod framing;
pub mod host;
pub mod listener;
pub mod server;
pub mod socket_path;
