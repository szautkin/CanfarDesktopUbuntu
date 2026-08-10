//! End-to-end connectivity tests: a real MCP client completing the handshake
//! against a real, socket-bound server — the exact path Claude Desktop / Claude
//! Code drive over `verbinal mcp`.
//!
//! The per-module unit tests already cover the pieces in isolation:
//! [`server`](crate::mcp::server) drives `serve` over an in-memory duplex,
//! [`framing`](crate::mcp::framing) round-trips the NDJSON codec, and
//! [`selftest`](crate::mcp::selftest) exercises the client codec over a duplex.
//! What was missing — and what these cover — is the two halves MEETING over a
//! real `AF_UNIX` kernel socket, through the production
//! [`listener::run_on_path`](crate::mcp::listener::run_on_path) accept loop and
//! the LIVE tool catalog ([`build_router`]). A pass here is the in-repo proof of
//! the module's headline promise: a real MCP client (Claude Desktop) will connect.
//!
//! None of these tests touch `XDG_RUNTIME_DIR`: the socket path is injected, so
//! they never race the user's live per-user socket and never collide with each
//! other. The shipped `verbinal mcp` bridge *binary* is covered separately by
//! `tests/mcp_bridge_stdio.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Runtime;

use crate::mcp::constants::SERVER_NAME;
use crate::mcp::framing::{read_frame, write_frame};
use crate::mcp::listener;
use crate::mcp::selftest::run_self_test_at;
use crate::mcp::server::{AllowAllGate, ApprovalGate};
use crate::mcp::tools::catalog::build_router;
use crate::mcp::tools::ToolRouter;
use crate::state::AppServices;

/// A fresh, private temp dir for one test's socket (unique per test + pid so
/// parallel test threads never share a socket node).
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("verbinal-e2e-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Poll-connect to `path` until it succeeds or `timeout` elapses, so a test only
/// proceeds once the spawned listener has actually bound the socket (bind races
/// the accept loop otherwise).
async fn wait_until_bound(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if UnixStream::connect(path).await.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "listener never bound {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Read the next non-empty NDJSON frame and parse it (skips keep-alive blanks).
async fn read_json<R: AsyncBufRead + Unpin>(r: &mut R) -> Value {
    loop {
        match read_frame(r).await.unwrap() {
            None => panic!("unexpected EOF while awaiting a reply frame"),
            Some(bytes) if bytes.is_empty() => continue,
            Some(bytes) => return serde_json::from_slice(&bytes).unwrap(),
        }
    }
}

/// A minimal MCP client: connect, `initialize` (asserting the server echoes our
/// protocolVersion and reports its name), then `tools/list`, returning the tool
/// names it advertises. This is what an MCP client does on connect.
async fn handshake_client(path: &Path) -> Vec<String> {
    let stream = UnixStream::connect(path).await.expect("client connect");
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);

    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "e2e-client", "version": "1" }
        }
    });
    write_frame(&mut wr, &serde_json::to_vec(&init).unwrap())
        .await
        .unwrap();
    let init_reply = read_json(&mut reader).await;
    // The server must echo the client's protocol version, never pin its own.
    assert_eq!(init_reply["result"]["protocolVersion"], json!("2024-11-05"));
    assert_eq!(
        init_reply["result"]["serverInfo"]["name"],
        json!(SERVER_NAME)
    );

    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    write_frame(&mut wr, &serde_json::to_vec(&list).unwrap())
        .await
        .unwrap();
    let list_reply = read_json(&mut reader).await;
    list_reply["result"]["tools"]
        .as_array()
        .expect("tools/list result must carry a tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect()
}

/// The Verify-step self-test client (the very code the connect wizard runs to
/// tell the user "Claude Desktop will connect") must complete its full
/// `initialize` → `tools/list` handshake against a real, production-bound
/// listener carrying the live catalog.
#[test]
fn self_test_client_round_trips_against_the_production_listener() {
    let dir = unique_dir("selftest");
    let path = dir.join("verbinal-mcp.sock");

    let rt = Runtime::new().unwrap();
    let (services, _toast_rx) = AppServices::new(rt.handle().clone());

    let result = rt.block_on(async {
        let (router, _proposals) = build_router(services);
        let router: Arc<dyn ToolRouter> = router;
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);

        // Production accept loop, on a private socket instead of the XDG one.
        let server = tokio::spawn(listener::run_on_path(path.clone(), router, gate));
        wait_until_bound(&path, Duration::from_secs(3)).await;

        let r = run_self_test_at(path.clone()).await;
        server.abort();
        r
    });

    let _ = std::fs::remove_dir_all(&dir);

    assert!(result.ok, "handshake failed: {:?}", result.error);
    assert_eq!(result.server_name.as_deref(), Some(SERVER_NAME));
    assert!(
        result.tool_count.unwrap_or(0) > 0,
        "the live catalog should expose at least one tool over the wire"
    );
}

/// Two independent clients must each complete a handshake on the same running
/// server — the per-connection-`serve` contract the listener promises (one client
/// can connect while another is still being served). Both must also see the
/// agent-safe lifecycle tools in `tools/list`, proving the real catalog is wired
/// through the socket, not just an empty manifest.
#[test]
fn two_clients_handshake_concurrently_over_the_listener() {
    let dir = unique_dir("concurrent");
    let path = dir.join("verbinal-mcp.sock");

    let rt = Runtime::new().unwrap();
    let (services, _toast_rx) = AppServices::new(rt.handle().clone());

    rt.block_on(async {
        let (router, _proposals) = build_router(services);
        let router: Arc<dyn ToolRouter> = router;
        let gate: Arc<dyn ApprovalGate> = Arc::new(AllowAllGate);

        let server = tokio::spawn(listener::run_on_path(path.clone(), router, gate));
        wait_until_bound(&path, Duration::from_secs(3)).await;

        let (a, b) = tokio::join!(handshake_client(&path), handshake_client(&path));
        server.abort();

        for (who, tools) in [("A", a), ("B", b)] {
            assert!(
                tools.iter().any(|t| t == "list_pending_proposals"),
                "client {who} should see the lifecycle tools in tools/list; got {tools:?}"
            );
        }
    });

    let _ = std::fs::remove_dir_all(&dir);
}
