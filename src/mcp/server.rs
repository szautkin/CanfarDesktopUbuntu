//! Per-connection MCP dispatch. Ported from `Mcp/McpServerService.cs`.
//!
//! ONE `serve` invocation per accepted connection. It serves the MCP method set
//! over an NDJSON byte stream: `initialize` (echoes the client's protocolVersion,
//! runs the approval gate), `tools/list` / `tools/call` (via the router, gated on
//! `initialize`), `ping` / `logging/setLevel` (empty ok), `resources/*` (empty),
//! anything else → methodNotFound. Notifications (absent `id`) get no reply;
//! `tools/*` before `initialize` → serverNotInitialized.
//!
//! Requests are dispatched CONCURRENTLY (a slow backend query must not stall the
//! other requests queued behind it); responses are matched by id, so out-of-order
//! completion is fine. A single writer task serializes the actual frame writes.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;

use crate::mcp::constants::{SERVER_NAME, SERVER_VERSION};
use crate::mcp::framing;
use crate::mcp::jsonrpc::{self, error_code, JsonRpcId, JsonRpcRequest};
use crate::mcp::tools::{ToolContext, ToolResult, ToolRouter};

/// Gate that decides whether a connecting client may proceed past `initialize`.
///
/// Mirrors the C# `IApprovalGate`. The real app pops a per-client consent prompt;
/// the trusted-socket / test default admits everyone (see [`AllowAllGate`]).
pub trait ApprovalGate: Send + Sync {
    fn permit<'a>(&'a self, client_id: &'a str) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}

/// A gate that admits every client. Used on the trusted local socket and in tests.
pub struct AllowAllGate;

impl ApprovalGate for AllowAllGate {
    fn permit<'a>(&'a self, _client_id: &'a str) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
}

/// Per-connection state shared (by `Arc`) with every spawned request handler.
struct ConnState {
    /// Set once `initialize` succeeds; `tools/*` before this → serverNotInitialized.
    initialized: AtomicBool,
    /// The connecting client's id (from `clientInfo.name`), stamped at `initialize`.
    client_id: Mutex<String>,
}

/// Serve the MCP protocol over one connection until EOF or a transport error.
///
/// Splits `stream`, wraps the read half in a `BufReader`, and runs a dedicated
/// writer task that drains a channel of encoded response frames (serializing all
/// writes). The read loop pulls one NDJSON frame at a time and spawns a handler
/// per non-empty frame; each handler computes its response document and pushes the
/// encoded bytes to the writer channel.
pub async fn serve<S>(
    stream: S,
    router: Arc<dyn ToolRouter>,
    gate: Arc<dyn ApprovalGate>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    // Writer task: the sole owner of the write half, so every response frame is
    // written atomically w.r.t. every other. Ends when the channel closes.
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer_task = tokio::spawn(async move {
        let mut write_half = write_half;
        while let Some(bytes) = rx.recv().await {
            if framing::write_frame(&mut write_half, &bytes).await.is_err() {
                break; // peer went away; stop draining
            }
        }
    });

    let state = Arc::new(ConnState {
        initialized: AtomicBool::new(false),
        client_id: Mutex::new(String::from("unknown-client")),
    });

    // Track in-flight handlers so we can drain them on a clean shutdown; prune
    // finished ones each iteration to bound the vector (mirrors the C# RemoveAll).
    let mut inflight: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    loop {
        let frame = match framing::read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,  // clean EOF
            Err(_) => break,    // transport/decode error → end connection
        };
        if frame.is_empty() {
            continue; // keep-alive
        }

        let router = Arc::clone(&router);
        let gate = Arc::clone(&gate);
        let state = Arc::clone(&state);
        let tx = tx.clone();

        inflight.push(tokio::spawn(async move {
            if let Some(response) = handle_frame(&frame, &router, &gate, &state).await {
                // serde_json::to_vec is single-line (no embedded '\n'); the frame
                // codec appends the terminator. A closed channel just means the
                // connection is ending — drop the response.
                if let Ok(bytes) = serde_json::to_vec(&response) {
                    let _ = tx.send(bytes);
                }
            }
        }));
        inflight.retain(|h| !h.is_finished());
    }

    // Let outstanding handlers finish (their responses reach the still-open writer),
    // then drop our sender so the writer task observes the closed channel and exits.
    for h in inflight {
        let _ = h.await;
    }
    drop(tx);
    let _ = writer_task.await;
    Ok(())
}

/// Process one incoming document; returns the response document, or `None` for a
/// notification (no reply). Mirrors `McpServerService.HandleFrameAsync`.
async fn handle_frame(
    frame: &[u8],
    router: &Arc<dyn ToolRouter>,
    gate: &Arc<dyn ApprovalGate>,
    state: &Arc<ConnState>,
) -> Option<Value> {
    let root: Value = match serde_json::from_slice(frame) {
        Ok(v) => v,
        Err(_) => return Some(jsonrpc::parse_error("parse error")),
    };

    let request = match JsonRpcRequest::parse(&root) {
        Ok(r) => r,
        Err(msg) => {
            return Some(jsonrpc::failure(
                &Some(JsonRpcId::Null),
                error_code::INVALID_REQUEST,
                &msg,
            ));
        }
    };

    if request.is_notification() {
        return None; // notifications/initialized etc. → no reply
    }

    Some(dispatch(&request, router, gate, state).await)
}

/// Route a parsed request to its handler. Mirrors `McpServerService.DispatchAsync`.
async fn dispatch(
    req: &JsonRpcRequest,
    router: &Arc<dyn ToolRouter>,
    gate: &Arc<dyn ApprovalGate>,
    state: &Arc<ConnState>,
) -> Value {
    match req.method.as_str() {
        "initialize" => handle_initialize(req, gate, state).await,
        "ping" => jsonrpc::success(&req.id, json!({})),
        "logging/setLevel" => jsonrpc::success(&req.id, json!({})),
        "tools/list" => match require_initialized(req, state) {
            Some(err) => err,
            None => handle_tools_list(req, router),
        },
        "tools/call" => match require_initialized(req, state) {
            Some(err) => err,
            None => handle_tools_call(req, router, state).await,
        },
        "resources/list" => match require_initialized(req, state) {
            Some(err) => err,
            None => jsonrpc::success(&req.id, json!({ "resources": [] })),
        },
        "resources/read" => match require_initialized(req, state) {
            Some(err) => err,
            None => jsonrpc::failure(
                &req.id,
                error_code::INVALID_PARAMS,
                "resources/read is not supported",
            ),
        },
        other => jsonrpc::failure(
            &req.id,
            error_code::METHOD_NOT_FOUND,
            &format!("method not found: {}", other),
        ),
    }
}

/// `None` if this connection is initialized; otherwise the serverNotInitialized error.
fn require_initialized(req: &JsonRpcRequest, state: &Arc<ConnState>) -> Option<Value> {
    if state.initialized.load(Ordering::SeqCst) {
        None
    } else {
        Some(jsonrpc::failure(
            &req.id,
            error_code::SERVER_NOT_INITIALIZED,
            "Server has not been initialized.",
        ))
    }
}

async fn handle_initialize(
    req: &JsonRpcRequest,
    gate: &Arc<dyn ApprovalGate>,
    state: &Arc<ConnState>,
) -> Value {
    let params = match &req.params {
        Some(p) => p,
        None => return jsonrpc::failure(&req.id, error_code::INVALID_PARAMS, "missing params"),
    };
    let obj = match params.as_object() {
        Some(o) => o,
        None => {
            return jsonrpc::failure(
                &req.id,
                error_code::INVALID_PARAMS,
                "initialize params must be an object",
            )
        }
    };
    // Echo the client's protocolVersion — never pin a server constant.
    let protocol = match obj.get("protocolVersion").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return jsonrpc::failure(
                &req.id,
                error_code::INVALID_PARAMS,
                "initialize params missing protocolVersion",
            )
        }
    };
    let client_id = obj
        .get("clientInfo")
        .and_then(|ci| ci.get("name"))
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown-client")
        .to_string();

    if !gate.permit(&client_id).await {
        return jsonrpc::failure(
            &req.id,
            error_code::SESSION_NOT_APPROVED,
            "Client not approved by user.",
        );
    }

    // Record client id + flip initialized before the reply is sent, so any request
    // the peer pipelines after seeing this response finds the connection ready.
    *state.client_id.lock().unwrap() = client_id;
    state.initialized.store(true, Ordering::SeqCst);

    jsonrpc::success(
        &req.id,
        json!({
            "protocolVersion": protocol,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        }),
    )
}

fn handle_tools_list(req: &JsonRpcRequest, router: &Arc<dyn ToolRouter>) -> Value {
    let tools: Vec<Value> = router
        .external_manifest()
        .into_iter()
        .map(|d| {
            json!({
                "name": d.name,
                "description": d.description,
                "inputSchema": d.input_schema,
            })
        })
        .collect();
    jsonrpc::success(&req.id, json!({ "tools": tools }))
}

async fn handle_tools_call(
    req: &JsonRpcRequest,
    router: &Arc<dyn ToolRouter>,
    state: &Arc<ConnState>,
) -> Value {
    let params = match &req.params {
        Some(p) => p,
        None => return jsonrpc::failure(&req.id, error_code::INVALID_PARAMS, "missing params"),
    };
    let obj = match params.as_object() {
        Some(o) => o,
        None => {
            return jsonrpc::failure(
                &req.id,
                error_code::INVALID_PARAMS,
                "tools/call params must be an object",
            )
        }
    };
    let name = match obj.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return jsonrpc::failure(
                &req.id,
                error_code::INVALID_PARAMS,
                "tools/call params missing name",
            )
        }
    };
    // Absent arguments default to an empty object.
    let args = obj.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let client_id = state.client_id.lock().unwrap().clone();
    let request_id = uuid::Uuid::new_v4().to_string();
    let ctx = ToolContext::for_external(client_id, request_id);

    let result = router.dispatch(&name, args, &ctx).await;
    jsonrpc::success(&req.id, map_tool_result(result))
}

/// Map a [`ToolResult`] to the `tools/call` result envelope
/// (`{ "content": [...], "isError": bool }`). Mirrors `MapToolResult`.
fn map_tool_result(result: ToolResult) -> Value {
    match result {
        ToolResult::Data(v) => json!({
            "content": [ { "type": "text", "text": v.to_string() } ],
            "isError": false,
        }),
        ToolResult::Text(s) => json!({
            "content": [ { "type": "text", "text": s } ],
            "isError": false,
        }),
        ToolResult::Failed(m) => json!({
            "content": [ { "type": "text", "text": m } ],
            "isError": true,
        }),
        ToolResult::Proposed(p) => {
            // The envelope an agent gets for a queued (not-yet-applied) write.
            let envelope = json!({
                "queued": true,
                "proposalId": p.id,
                "kind": p.kind,
                "summary": p.summary,
                "state": "pending",
            });
            json!({
                "content": [ { "type": "text", "text": envelope.to_string() } ],
                "isError": false,
            })
        }
        ToolResult::Image { data_base64, mime, caption } => {
            let mut content = vec![json!({
                "type": "image",
                "data": data_base64,
                "mimeType": mime,
            })];
            if let Some(c) = caption {
                if !c.is_empty() {
                    content.push(json!({ "type": "text", "text": c }));
                }
            }
            json!({ "content": content, "isError": false })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::NullRouter;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    async fn read_line(reader: &mut (impl AsyncBufReadExt + Unpin)) -> Value {
        let mut buf = Vec::new();
        reader.read_until(b'\n', &mut buf).await.unwrap();
        assert!(!buf.is_empty(), "expected a response frame, got EOF");
        serde_json::from_slice(&buf).unwrap()
    }

    async fn send(writer: &mut (impl AsyncWriteExt + Unpin), doc: &Value) {
        let mut line = serde_json::to_vec(doc).unwrap();
        assert!(!line.contains(&b'\n'), "frame must be single-line NDJSON");
        line.push(b'\n');
        writer.write_all(&line).await.unwrap();
        writer.flush().await.unwrap();
    }

    #[tokio::test]
    async fn initialize_then_tools_list() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let router: Arc<dyn ToolRouter> = Arc::new(NullRouter);
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);

        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);

        // initialize — the server must echo our protocolVersion, not pin one.
        send(
            &mut wr,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": { "name": "test-client", "version": "0.1" }
                }
            }),
        )
        .await;
        let init_resp = read_line(&mut reader).await;
        assert_eq!(init_resp["id"], json!(1));
        assert_eq!(init_resp["result"]["protocolVersion"], json!("2024-11-05"));
        assert_eq!(init_resp["result"]["serverInfo"]["name"], json!(SERVER_NAME));

        // tools/list — NullRouter exposes nothing, so the array is empty.
        send(
            &mut wr,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        )
        .await;
        let list_resp = read_line(&mut reader).await;
        assert_eq!(list_resp["id"], json!(2));
        assert_eq!(list_resp["result"]["tools"], json!([]));

        // Drop BOTH client halves so the duplex fully closes and the server's read
        // side sees EOF; otherwise `serve` loops forever and the await below hangs.
        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn tools_list_before_initialize_is_not_initialized() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let router: Arc<dyn ToolRouter> = Arc::new(NullRouter);
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);

        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);

        send(
            &mut wr,
            &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list", "params": {} }),
        )
        .await;
        let resp = read_line(&mut reader).await;
        assert_eq!(resp["error"]["code"], json!(error_code::SERVER_NOT_INITIALIZED));

        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn notification_gets_no_reply_then_ping_answers() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let router: Arc<dyn ToolRouter> = Arc::new(NullRouter);
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);

        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);

        // A notification (no id) must produce no response frame.
        send(
            &mut wr,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        // ping is answerable pre-initialize; its reply is the next frame we see,
        // proving the notification above did not emit one.
        send(&mut wr, &json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" })).await;

        let resp = read_line(&mut reader).await;
        assert_eq!(resp["id"], json!(5));
        assert_eq!(resp["result"], json!({}));

        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }
}
