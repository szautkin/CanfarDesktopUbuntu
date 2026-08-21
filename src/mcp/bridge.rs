//! The `verbinal mcp` stdio↔socket bridge (the Linux equivalent of the Windows
//! console `BridgeRelay`).
//!
//! An MCP client (Claude Desktop / Claude Code) launches a thin subprocess and
//! talks to it over stdio. That subprocess is this bridge: it connects to the
//! in-app listener's UNIX socket (see [`crate::mcp::socket_path`]) and pumps
//! complete NDJSON frames both ways — stdin → socket and socket → stdout — until
//! either side closes.
//!
//! Because framing is line-delimited and we relay raw bytes, no parsing is
//! needed here; [`tokio::io::copy`] moves the bytes verbatim. When the app is
//! not running the initial `connect` fails (`ENOENT`/`ECONNREFUSED`); that is a
//! normal condition, so we simply return the I/O error to the caller rather
//! than trying to answer requests ourselves.
//!
//! Mirrors `Mcp/Bridge/BridgeRelay.cs::RelayAsync`.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::mcp::socket_path::socket_path;

/// How long the bridge waits for the app's listener to come up after launching it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Connect to the in-app listener and relay stdio ↔ socket until either end
/// closes.
///
/// If the initial connect fails — the app isn't running, or a stale socket file
/// is left over from a previous session — the bridge launches the Verbinal app
/// (which auto-starts the MCP server when the user left it enabled) and retries
/// for a bounded window, so an AI client can connect without the app already
/// being open. The connect error is only surfaced if that window elapses.
pub async fn run_stdio_bridge() -> io::Result<()> {
    let path = socket_path();
    let stream = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(_) => {
            launch_app_detached();
            connect_with_retry(&path, CONNECT_TIMEOUT).await?
        }
    };
    relay(tokio::io::stdin(), tokio::io::stdout(), stream).await
}

/// Spawn the Verbinal GUI (no args) detached from this bridge's stdio so it can
/// bind the control socket. Single-instance by app-id, so a running app just gets
/// re-activated rather than duplicated. Best-effort — the retry loop reports any
/// real failure.
fn launch_app_detached() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Poll-connect to `path` until it succeeds or `timeout` elapses (a stale socket
/// yields `ECONNREFUSED` until the app re-binds it).
async fn connect_with_retry(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(path).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

/// Pump bytes in both directions between a stdio pair (`input`/`output`) and a
/// duplex `sock`, returning as soon as either direction completes — the
/// `Task.WhenAny` semantics of the reference relay.
///
/// Generic over the three streams so it can be exercised with in-memory duplex
/// pipes in tests without touching real stdio or a socket.
async fn relay<In, Out, Sock>(mut input: In, mut output: Out, sock: Sock) -> io::Result<()>
where
    In: AsyncRead + Unpin,
    Out: AsyncWrite + Unpin,
    Sock: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sock_rd, mut sock_wr) = tokio::io::split(sock);

    // socket → stdout
    let inbound = async {
        let r = tokio::io::copy(&mut sock_rd, &mut output).await;
        // Server closed / stopped sending: flush and close stdout.
        let _ = output.shutdown().await;
        r
    };

    // stdin → socket
    let outbound = async {
        let r = tokio::io::copy(&mut input, &mut sock_wr).await;
        // EOF on stdin: half-close the socket write side so the server sees the
        // client is done sending yet can still flush its final response.
        let _ = sock_wr.shutdown().await;
        r
    };

    tokio::pin!(inbound, outbound);

    // Whichever direction ends first ends the bridge; the other future is
    // dropped, releasing its borrows.
    tokio::select! {
        r = &mut inbound => r.map(|_| ()),
        r = &mut outbound => r.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The bridge lets go when the APP goes away, not just when the client does.
    ///
    /// QA report #2 (P3-D) found bridges that never exit. They do exit on stdin
    /// EOF — the case already covered below — but Claude Desktop abandons a
    /// bridge on each retry WITHOUT closing its stdin, so that signal never
    /// arrives and four of them accumulated in fourteen minutes.
    ///
    /// This is the other end: when the app quits or the socket drops, the
    /// bridge must not sit holding a pipe forever waiting for a client that has
    /// stopped listening.
    #[tokio::test]
    async fn ends_when_the_socket_closes_even_if_stdin_stays_open() {
        let (bridge_input, _client_stdin) = tokio::io::duplex(64);
        let (bridge_output, _client_stdout) = tokio::io::duplex(64);
        let (bridge_sock, peer) = tokio::io::duplex(64);

        let handle = tokio::spawn(relay(bridge_input, bridge_output, bridge_sock));

        // The app exits: its side of the socket goes away. `_client_stdin` is
        // deliberately still held, so stdin never reaches EOF.
        drop(peer);

        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(
            ended.is_ok(),
            "the bridge outlived its socket — this is the process that leaks"
        );
    }

    /// A client that closes its read end takes the bridge with it.
    ///
    /// The other way a client can disappear without closing stdin: it stops
    /// reading. The next relayed frame fails with a broken pipe, and that is a
    /// signal worth acting on rather than retrying forever.
    #[tokio::test]
    async fn ends_when_the_client_stops_reading() {
        let (bridge_input, _client_stdin) = tokio::io::duplex(64);
        let (bridge_output, client_stdout) = tokio::io::duplex(64);
        let (bridge_sock, mut peer) = tokio::io::duplex(64);

        let handle = tokio::spawn(relay(bridge_input, bridge_output, bridge_sock));

        // The client is gone; nothing will read stdout again.
        drop(client_stdout);
        // The app answers anyway. Enough to overrun the pipe buffer.
        let _ = peer.write_all(&vec![b'x'; 8192]).await;

        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(
            ended.is_ok(),
            "the bridge kept relaying into a pipe nobody reads"
        );
    }

    #[tokio::test]
    async fn relays_both_directions_then_ends_on_stdin_eof() {
        // Each duplex pair: writing one end appears on the other end's reads.
        let (bridge_input, mut client_stdin) = tokio::io::duplex(64);
        let (bridge_output, mut client_stdout) = tokio::io::duplex(64);
        let (bridge_sock, mut peer) = tokio::io::duplex(64);

        let handle = tokio::spawn(relay(bridge_input, bridge_output, bridge_sock));

        // stdin → socket
        client_stdin.write_all(b"hello").await.unwrap();
        let mut a = [0u8; 5];
        peer.read_exact(&mut a).await.unwrap();
        assert_eq!(&a, b"hello");

        // socket → stdout
        peer.write_all(b"world").await.unwrap();
        let mut b = [0u8; 5];
        client_stdout.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"world");

        // Closing stdin ends the outbound copy, so the bridge returns.
        drop(client_stdin);
        let result = handle.await.expect("bridge task panicked");
        assert!(result.is_ok(), "relay ended cleanly on EOF: {result:?}");
    }
}
