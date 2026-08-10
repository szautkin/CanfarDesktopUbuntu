//! End-to-end test of the SHIPPED `verbinal mcp` stdio bridge — the exact
//! process an MCP client (Claude Desktop / Claude Code) launches.
//!
//! Claude Desktop's `claude_desktop_config.json` runs `verbinal mcp`, which
//! connects to the app's per-user UNIX socket and relays JSON-RPC over the
//! child's stdio (see `src/mcp/bridge.rs`). Here we stand in for the app: we bind
//! that socket ourselves, launch the real compiled binary in bridge mode pointed
//! at a private socket via `$VERBINAL_MCP_SOCKET`, and drive a full
//! `initialize` then `tools/call` exchange THROUGH the bridge — proving the
//! shipped binary connects to the socket, relays both directions faithfully,
//! and exits cleanly on stdin EOF.
//!
//! Isolation MUST use `$VERBINAL_MCP_SOCKET`, not `$XDG_RUNTIME_DIR`: the
//! uid-derived `/run/user/<uid>` path deliberately outranks XDG (see
//! `src/mcp/socket_path.rs`), so on any systemd host an XDG override would send
//! the bridge to the developer's own running app instead of our stand-in.
//!
//! Unlike the in-process tests in `src/mcp/e2e_tests.rs`, this exercises the
//! actual `main() -> mcp::bridge::run_stdio_bridge()` entry point end to end
//! across a process boundary. `CARGO_BIN_EXE_verbinal` is provided to
//! integration tests by Cargo.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// A private temp dir holding the socket this test owns — never the developer's
/// real per-user one.
fn unique_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("verbinal-bridge-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `value` as one NDJSON frame: a single-line JSON document + `\n`, flushed.
fn write_frame<W: Write>(w: &mut W, value: &Value) {
    let mut line = serde_json::to_string(value).unwrap();
    assert!(!line.contains('\n'), "a frame must be single-line NDJSON");
    line.push('\n');
    w.write_all(line.as_bytes()).unwrap();
    w.flush().unwrap();
}

/// Read the next non-empty NDJSON frame and parse it (skips keep-alive blanks).
/// Panics on EOF so a dead bridge fails the test loudly instead of hanging.
fn read_frame<R: BufRead>(r: &mut R) -> Value {
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).expect("read a frame line");
        assert!(n != 0, "unexpected EOF while awaiting a JSON reply frame");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // keep-alive blank line
        }
        return serde_json::from_str(trimmed).expect("reply must be valid JSON");
    }
}

/// Wait for `child` to exit within `timeout`, or kill it and fail. Returns the
/// exit status. Guards against a hung bridge blocking the whole test suite.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> std::process::ExitStatus {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let status = child.wait();
        // If the wait raced the timeout branch, `send` just fails harmlessly.
        let _ = tx.send((status, child));
    });
    match rx.recv_timeout(timeout) {
        Ok((status, _child)) => {
            handle.join().ok();
            status.expect("wait() on the bridge failed")
        }
        Err(_) => panic!("bridge did not exit within {timeout:?} of stdin EOF"),
    }
}

#[test]
fn bridge_binary_relays_full_mcp_exchange_and_exits_on_eof() {
    let dir = unique_dir();
    let sock = dir.join("verbinal-mcp.sock");
    let _ = std::fs::remove_file(&sock);

    // --- stand in for the running app: bind the control socket and answer MCP ---
    // bind() puts the socket in the listening state immediately, so the bridge's
    // connect() succeeds even before accept() runs — no bind/connect race, and the
    // bridge never falls back to launching the GUI.
    let listener = UnixListener::bind(&sock).expect("bind the control socket");
    let server = thread::spawn(move || {
        let (conn, _addr) = listener.accept().expect("the bridge should connect");
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut writer: UnixStream = conn;

        // initialize → reply that echoes the client's protocolVersion.
        let req1 = read_frame(&mut reader);
        assert_eq!(req1["method"], "initialize");
        write_frame(
            &mut writer,
            &json!({
                "jsonrpc": "2.0", "id": req1["id"],
                "result": {
                    "protocolVersion": req1["params"]["protocolVersion"],
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "verbinal", "version": "test" }
                }
            }),
        );

        // tools/call → a simple text result.
        let req2 = read_frame(&mut reader);
        assert_eq!(req2["method"], "tools/call");
        write_frame(
            &mut writer,
            &json!({
                "jsonrpc": "2.0", "id": req2["id"],
                "result": { "content": [ { "type": "text", "text": "pong" } ], "isError": false }
            }),
        );

        // The bridge half-closes the socket write side when its stdin hits EOF;
        // draining to EOF confirms that clean shutdown and ends this thread.
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });

    // --- the real bridge subprocess, exactly as Claude Desktop spawns it ---
    let exe = env!("CARGO_BIN_EXE_verbinal");
    let mut child = Command::new(exe)
        .arg("mcp")
        .env("VERBINAL_MCP_SOCKET", &sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch `verbinal mcp`");

    let mut child_in = child.stdin.take().unwrap();
    let mut child_out = BufReader::new(child.stdout.take().unwrap());

    // initialize, through the bridge.
    write_frame(
        &mut child_in,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": "claude-desktop-e2e", "version": "1" }
            }
        }),
    );
    let init = read_frame(&mut child_out);
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init["result"]["serverInfo"]["name"], "verbinal");

    // tools/call, through the bridge — proves relaying works after initialize too.
    // The stand-in server above scripts the reply, so this asserts RELAYING, not
    // the tool catalog (which `src/mcp/e2e_tests.rs` covers against the real router).
    write_frame(
        &mut child_in,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "ping", "arguments": {} }
        }),
    );
    let call = read_frame(&mut child_out);
    assert_eq!(call["id"], 2);
    assert_eq!(call["result"]["content"][0]["text"], "pong");

    // EOF on the bridge's stdin → it half-closes the socket and exits 0.
    drop(child_in);
    let status = wait_with_timeout(child, Duration::from_secs(10));
    assert!(status.success(), "bridge exited with failure: {status:?}");

    server.join().expect("server thread panicked");
    let _ = std::fs::remove_dir_all(&dir);
}
