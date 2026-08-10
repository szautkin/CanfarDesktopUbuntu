//! Live round-trip self-test against the running MCP server.
//!
//! Linux port of `CanfarDesktop/Mcp/McpSelfTest.cs` + `McpSelfTestProtocol.cs`.
//! Instead of proving only "the toggle is on", we dial the real per-user UNIX
//! domain socket the bridge uses, `initialize`, and `tools/list` — a genuine
//! client handshake. A pass here means a real MCP client (Claude Desktop /
//! Claude Code) will connect too, because we reuse the exact
//! [`socket_path`](crate::mcp::socket_path::socket_path) and the exact NDJSON
//! framing ([`read_frame`]/[`write_frame`]) the server speaks.
//!
//! Timeouts mirror the reference: a 2-second connect bound and a 4-second bound
//! over the whole handshake. Any failure — unreachable socket, no reply,
//! malformed frame — collapses to `SelfTestResult { ok: false, error: Some(..) }`
//! with a message fit to show a user.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::net::UnixStream;

use crate::mcp::framing::{read_frame, write_frame};
use crate::mcp::socket_path::socket_path;

/// How long we wait to establish the socket connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Overall bound on the full connect + handshake.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(4);

/// The `clientInfo.name` we announce, so the server-side logs/audit can tell a
/// self-test connection apart from a real agent.
const SELF_TEST_CLIENT: &str = "verbinal-selftest";

/// Outcome of a live self-test: did a client reach the running server, and what
/// did it see. `ok` is the analogue of the reference `Reachable`.
#[derive(Debug, Clone)]
pub struct SelfTestResult {
    /// The handshake completed (connect + `initialize` reply). `tools/list`
    /// count is best-effort and may be `None` even when `ok`.
    pub ok: bool,
    /// `serverInfo.name` from the `initialize` reply, if the server sent one.
    pub server_name: Option<String>,
    /// Number of tools in the `tools/list` reply, if it arrived and parsed.
    pub tool_count: Option<usize>,
    /// A user-facing explanation when `ok` is false.
    pub error: Option<String>,
}

impl SelfTestResult {
    /// A failed result carrying a user-facing message.
    fn failed(message: impl Into<String>) -> Self {
        SelfTestResult {
            ok: false,
            server_name: None,
            tool_count: None,
            error: Some(message.into()),
        }
    }
}

/// Dial the running MCP server and drive a full `initialize` → `tools/list`
/// round-trip. Never panics; every failure path returns a `SelfTestResult`
/// whose `error` is safe to surface in the connect wizard's Verify step.
pub async fn run_self_test() -> SelfTestResult {
    run_self_test_at(socket_path()).await
}

/// [`run_self_test`] against an explicit socket `path`.
///
/// A test seam: it lets the full connect + handshake run over a private temp
/// socket without mutating the process-wide `XDG_RUNTIME_DIR`. Production always
/// reaches this through [`run_self_test`] with [`socket_path()`], so the wizard's
/// Verify step is unchanged.
pub(crate) async fn run_self_test_at(path: PathBuf) -> SelfTestResult {
    match tokio::time::timeout(OVERALL_TIMEOUT, handshake(path)).await {
        Ok(result) => result,
        Err(_) => SelfTestResult::failed(
            "The MCP server didn't finish the handshake in time. Make sure it's enabled and try again.",
        ),
    }
}

/// The connect + handshake, bounded overall by [`run_self_test`]'s 4s timeout.
async fn handshake(path: PathBuf) -> SelfTestResult {
    // Connect with its own 2s bound (Ok(Ok) = connected; Ok(Err) = refused;
    // Err = the 2s elapsed).
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&path)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            return SelfTestResult::failed(format!(
                "Couldn't reach the MCP server at {}: {err}. Make sure it's enabled and try again.",
                path.display()
            ));
        }
        Err(_) => {
            return SelfTestResult::failed(
                "Couldn't reach the MCP server in time. Make sure it's enabled and try again.",
            );
        }
    };

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // 1) initialize
    let init_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": SELF_TEST_CLIENT, "version": "1" }
        }
    });
    if let Err(err) = send(&mut write_half, &init_request).await {
        return SelfTestResult::failed(format!("Sending initialize failed: {err}"));
    }

    let init_reply = match recv(&mut reader).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return SelfTestResult::failed(
                "The server accepted the connection but didn't answer initialize.",
            );
        }
        Err(err) => {
            return SelfTestResult::failed(format!("Reading the initialize reply failed: {err}"));
        }
    };
    let server_name = parse_server_name(&init_reply);

    // 2) notifications/initialized (no reply expected)
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    if let Err(err) = send(&mut write_half, &initialized).await {
        return SelfTestResult::failed(format!("Sending the initialized notification failed: {err}"));
    }

    // 3) tools/list — the count is a bonus; a missing/short reply doesn't fail
    //    the handshake (the connection itself is already proven).
    let list_request = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    if let Err(err) = send(&mut write_half, &list_request).await {
        return SelfTestResult::failed(format!("Sending tools/list failed: {err}"));
    }
    let tool_count = match recv(&mut reader).await {
        Ok(Some(value)) => parse_tool_count(&value),
        _ => None,
    };

    SelfTestResult {
        ok: true,
        server_name,
        tool_count,
        error: None,
    }
}

/// Serialize `value` to a single-line document and write it as one NDJSON frame.
async fn send<W>(writer: &mut W, value: &Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_frame(writer, &bytes).await
}

/// Read the next non-empty NDJSON frame and parse it as JSON. Empty frames
/// (keep-alives from [`read_frame`]) are skipped; `Ok(None)` means EOF.
async fn recv<R>(reader: &mut R) -> std::io::Result<Option<Value>>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        match read_frame(reader).await? {
            None => return Ok(None),
            Some(bytes) if bytes.is_empty() => continue, // keep-alive; ignore
            Some(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                return Ok(Some(value));
            }
        }
    }
}

/// `serverInfo.name` from an `initialize` reply, or `None` if the shape differs.
fn parse_server_name(reply: &Value) -> Option<String> {
    reply
        .get("result")?
        .get("serverInfo")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

/// `result.tools.len()` from a `tools/list` reply, or `None` if the shape differs.
fn parse_tool_count(reply: &Value) -> Option<usize> {
    reply.get("result")?.get("tools")?.as_array().map(|a| a.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_name_from_initialize_reply() {
        let reply = json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "serverInfo": { "name": "verbinal", "version": "1.3" } }
        });
        assert_eq!(parse_server_name(&reply).as_deref(), Some("verbinal"));
    }

    #[test]
    fn server_name_is_none_for_wrong_shape() {
        assert_eq!(parse_server_name(&json!({ "result": {} })), None);
        assert_eq!(parse_server_name(&json!({ "error": { "code": -1 } })), None);
        assert_eq!(parse_server_name(&json!({})), None);
    }

    #[test]
    fn parses_tool_count_from_tools_list_reply() {
        let reply = json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "tools": [ { "name": "a" }, { "name": "b" }, { "name": "c" } ] }
        });
        assert_eq!(parse_tool_count(&reply), Some(3));
    }

    #[test]
    fn tool_count_is_none_for_wrong_shape() {
        assert_eq!(parse_tool_count(&json!({ "result": { "tools": {} } })), None);
        assert_eq!(parse_tool_count(&json!({ "result": {} })), None);
        assert_eq!(parse_tool_count(&json!({})), None);
    }

    #[tokio::test]
    async fn send_and_recv_round_trip_over_duplex() {
        // A frame written by `send` is read back and parsed by `recv`.
        let (a, b) = tokio::io::duplex(256);
        let doc = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });

        let writer = tokio::spawn({
            let doc = doc.clone();
            async move {
                let mut w = a;
                send(&mut w, &doc).await.unwrap();
            }
        });

        let mut reader = BufReader::new(b);
        let got = recv(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, doc);

        writer.await.unwrap();
    }

    #[tokio::test]
    async fn recv_skips_keep_alive_frames() {
        // A blank NDJSON line is a keep-alive; recv should skip to the real doc.
        let (a, b) = tokio::io::duplex(256);
        let writer = tokio::spawn(async move {
            let mut w = a;
            // Two blank keep-alives, then a real document.
            write_frame(&mut w, b"").await.unwrap();
            write_frame(&mut w, b"").await.unwrap();
            write_frame(&mut w, br#"{"ok":true}"#).await.unwrap();
        });

        let mut reader = BufReader::new(b);
        let got = recv(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, json!({ "ok": true }));

        writer.await.unwrap();
    }

    #[tokio::test]
    async fn self_test_fails_gracefully_when_socket_absent() {
        // A socket path with nothing bound: connect fails fast and we still get a
        // well-formed failure result. The path is injected, so no XDG mutation is
        // needed (and no race with other env-sensitive tests).
        let path = std::env::temp_dir()
            .join(format!("verbinal-selftest-absent-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let result = run_self_test_at(path).await;

        assert!(!result.ok);
        assert!(result.error.is_some());
    }
}
