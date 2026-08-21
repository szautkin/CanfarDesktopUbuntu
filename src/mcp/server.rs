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
use tokio::sync::{broadcast, mpsc};

use crate::mcp::constants::{SERVER_NAME, SERVER_VERSION};

/// How long the tool list must stop changing before clients are told.
///
/// Long enough to swallow a burst — every AI Guide edit persists, and every
/// persist fires — and short enough that a single deliberate change still feels
/// immediate to whoever made it. The notification carries no payload beyond
/// "re-read the list", so collapsing several loses nothing at all.
const LIST_CHANGED_QUIET: std::time::Duration = std::time::Duration::from_millis(250);
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

#[cfg(test)]
/// A gate that admits every client. Used on the trusted local socket and in tests.
pub struct AllowAllGate;

#[cfg(test)]
impl ApprovalGate for AllowAllGate {
    fn permit<'a>(
        &'a self,
        _client_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
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

    // Tell this client when the tool list changes underneath it. Guide tools
    // are user-editable at any moment, so a name cached at connect time can
    // stop existing mid-session — the client's next call fails as "unknown
    // tool" with nothing to explain why.
    //
    // Held so it can be aborted when the connection ends; otherwise the task
    // would outlive its writer and sit on a broadcast forever.
    let notifier = router.subscribe_manifest_changed().map(|mut changed| {
        let tx = tx.clone();
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                match changed.recv().await {
                    Ok(()) => {}
                    // Lagged: events were dropped while this connection was
                    // busy. The payload is "re-read the list", so one
                    // notification says everything several would have.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }

                // Coalesce a burst into one notification. Every guide edit
                // persists, and each persist fires — so renaming a guide, or
                // any UI that saves per keystroke, would tell the client to
                // re-read a 141-tool catalogue once per event. Wait for the
                // churn to stop, draining whatever arrives meanwhile.
                let mut closed = false;
                loop {
                    match tokio::time::timeout(LIST_CHANGED_QUIET, changed.recv()).await {
                        // More churn: keep waiting for it to settle.
                        Ok(Ok(())) => continue,
                        Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                        Ok(Err(broadcast::error::RecvError::Closed)) => {
                            closed = true;
                            break;
                        }
                        // Quiet: send the one notification the burst earned.
                        Err(_) => break,
                    }
                }
                if closed {
                    break;
                }
                // Nothing before `initialize` — the client has not agreed a
                // protocol version yet, and an unsolicited frame there is a
                // spec violation rather than a helpful early warning.
                if !state.initialized.load(Ordering::SeqCst) {
                    continue;
                }
                let frame = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/tools/list_changed",
                });
                let Ok(bytes) = serde_json::to_vec(&frame) else {
                    continue;
                };
                if tx.send(bytes).is_err() {
                    break; // connection gone
                }
            }
        })
    });

    // Track in-flight handlers so we can drain them on a clean shutdown; prune
    // finished ones each iteration to bound the vector (mirrors the C# RemoveAll).
    let mut inflight: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    loop {
        let frame = match framing::read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break, // clean EOF
            Err(_) => break,   // transport/decode error → end connection
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
    // The notifier holds a CLONE of `tx` and waits on a broadcast that outlives
    // this connection, so dropping our own sender does not close the channel.
    // Without this the writer task below never sees the channel close and
    // `serve` never returns — the connection is not merely leaked, it never
    // finishes ending.
    if let Some(n) = notifier {
        n.abort();
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
            // `listChanged` is a promise, and it is kept below: the AI Guide's
            // tools are read live, so this server's list genuinely does change
            // while a client is connected.
            "capabilities": { "tools": { "listChanged": true } },
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
        ToolResult::Image {
            data_base64,
            mime,
            caption,
        } => {
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
        assert_eq!(
            init_resp["result"]["serverInfo"]["name"],
            json!(SERVER_NAME)
        );

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

    /// A router whose tool list can be made to change, like the real one's.
    struct ChangingRouter {
        changed: broadcast::Sender<()>,
    }

    impl ToolRouter for ChangingRouter {
        fn external_manifest(&self) -> Vec<crate::mcp::tools::ToolDescriptor> {
            Vec::new()
        }

        fn dispatch<'a>(
            &'a self,
            _name: &'a str,
            _args: Value,
            _ctx: &'a ToolContext,
        ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async { ToolResult::Failed("no tools".to_string()) })
        }

        fn subscribe_manifest_changed(&self) -> Option<broadcast::Receiver<()>> {
            Some(self.changed.subscribe())
        }
    }

    /// Wait until `serve` has subscribed its notifier.
    ///
    /// `send` on a broadcast with no receivers is an error, not a queued
    /// message, so firing before the spawned connection task has run is a race
    /// that fails the test rather than the code. Yielding until the subscriber
    /// appears makes the ordering a fact instead of a hope.
    async fn wait_for_subscriber(changed: &broadcast::Sender<()>) {
        for _ in 0..1_000 {
            if changed.receiver_count() > 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("serve never subscribed to manifest changes");
    }

    /// Drive a connection to the point where it has completed `initialize`.
    async fn initialized_client<S>(
        reader: &mut BufReader<tokio::io::ReadHalf<S>>,
        wr: &mut tokio::io::WriteHalf<S>,
    ) -> Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite,
    {
        send(
            wr,
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
        read_line(reader).await
    }

    #[tokio::test]
    async fn the_server_promises_to_report_tool_list_changes() {
        // A client has no reason to listen for the notification unless the
        // server said it sends it, so the promise and the delivery are one
        // feature. This half is the promise.
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (changed, _) = broadcast::channel(8);
        let router: Arc<dyn ToolRouter> = Arc::new(ChangingRouter {
            changed: changed.clone(),
        });
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);
        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);
        let init = initialized_client(&mut reader, &mut wr).await;

        assert_eq!(
            init["result"]["capabilities"]["tools"]["listChanged"],
            json!(true),
            "the tool list does change; the capability has to say so"
        );

        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn a_changed_tool_list_reaches_a_connected_client() {
        // The AI Guide's tools are read live on every `tools/list`, so adding
        // or removing one changes what this client may call. Without this frame
        // the next call fails as "unknown tool" and nothing explains why.
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (changed, _) = broadcast::channel(8);
        let router: Arc<dyn ToolRouter> = Arc::new(ChangingRouter {
            changed: changed.clone(),
        });
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);
        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);
        initialized_client(&mut reader, &mut wr).await;

        // The user edits their AI Guide.
        wait_for_subscriber(&changed).await;
        changed.send(()).expect("a live subscriber");

        // Bounded: a server that never sends it should FAIL here, not hang the
        // suite until CI's own timeout kills it with no useful message.
        let note = tokio::time::timeout(std::time::Duration::from_secs(5), read_line(&mut reader))
            .await
            .expect("no notifications/tools/list_changed arrived within 5s");
        assert_eq!(note["method"], json!("notifications/tools/list_changed"));
        assert!(
            note.get("id").is_none(),
            "a notification carries no id: {note}"
        );

        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn the_notifier_does_not_outlive_its_connection() {
        // It holds a clone of the connection's writer channel and waits on a
        // broadcast owned by the app, so dropping the connection's own sender
        // does not end it. One leaked task per connection, for as long as the
        // app runs, and each one still holding a receiver slot.
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (changed, _) = broadcast::channel(8);
        let router: Arc<dyn ToolRouter> = Arc::new(ChangingRouter {
            changed: changed.clone(),
        });
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);
        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);
        initialized_client(&mut reader, &mut wr).await;
        wait_for_subscriber(&changed).await;
        assert_eq!(changed.receiver_count(), 1, "the connection is subscribed");

        drop(reader);
        drop(wr);
        // Bounded, because the failure mode here is a hang: the notifier's
        // clone of the writer channel keeps it open, so `serve` waits on a
        // writer task that will never see it close.
        tokio::time::timeout(std::time::Duration::from_secs(5), server_task)
            .await
            .expect("serve never returned after the client hung up")
            .unwrap();

        // `serve` has returned. Nothing of this connection may still be
        // listening.
        for _ in 0..1_000 {
            if changed.receiver_count() == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the notifier is still subscribed after its connection closed");
    }

    /// A burst of edits produces one notification, not one each.
    ///
    /// QA report #2 (P3-D): re-registering 138 tools thrashes a client, and
    /// every AI Guide edit persists — so a rename, or any save-per-keystroke
    /// path, would fire once per event. The payload is only "re-read the list",
    /// so collapsing a burst loses nothing.
    #[tokio::test]
    async fn a_burst_of_changes_is_one_notification() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (changed, _) = broadcast::channel(64);
        let router: Arc<dyn ToolRouter> = Arc::new(ChangingRouter {
            changed: changed.clone(),
        });
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);
        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);
        initialized_client(&mut reader, &mut wr).await;
        wait_for_subscriber(&changed).await;

        // Ten edits in quick succession.
        for _ in 0..10 {
            changed.send(()).expect("a live subscriber");
        }

        let note = tokio::time::timeout(std::time::Duration::from_secs(5), read_line(&mut reader))
            .await
            .expect("the burst must still produce one notification");
        assert_eq!(note["method"], json!("notifications/tools/list_changed"));

        // And no second one: the other nine were coalesced into that.
        let extra = tokio::time::timeout(LIST_CHANGED_QUIET * 8, read_line(&mut reader)).await;
        assert!(
            extra.is_err(),
            "a burst of ten produced more than one notification: {extra:?}"
        );

        drop(reader);
        drop(wr);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_task).await;
    }

    #[tokio::test]
    async fn nothing_is_pushed_before_the_client_has_initialized() {
        // An unsolicited frame before the protocol version is agreed is a spec
        // violation, not an early warning.
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (changed, _) = broadcast::channel(8);
        let router: Arc<dyn ToolRouter> = Arc::new(ChangingRouter {
            changed: changed.clone(),
        });
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);
        let server_task = tokio::spawn(async move {
            let _ = serve(server, router, gate).await;
        });

        let (rd, mut wr) = tokio::io::split(client);
        let mut reader = BufReader::new(rd);

        // Fire the change while the connection is still pre-initialize, and
        // give the notifier a chance to act on it.
        wait_for_subscriber(&changed).await;
        changed.send(()).expect("a live subscriber");
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

        // The first frame the client ever sees must be its own initialize
        // reply, not a notification that arrived before the handshake.
        let first = initialized_client(&mut reader, &mut wr).await;
        assert_eq!(first["id"], json!(1), "got an unsolicited frame: {first}");

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
        assert_eq!(
            resp["error"]["code"],
            json!(error_code::SERVER_NOT_INITIALIZED)
        );

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
        send(
            &mut wr,
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
        )
        .await;

        let resp = read_line(&mut reader).await;
        assert_eq!(resp["id"], json!(5));
        assert_eq!(resp["result"], json!({}));

        drop(reader);
        drop(wr);
        server_task.await.unwrap();
    }
}
