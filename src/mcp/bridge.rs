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

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::mcp::socket_path::socket_path;

/// Connect to the in-app listener and relay stdio ↔ socket until either end
/// closes. Returns the connect error unchanged if the app is not running.
pub async fn run_stdio_bridge() -> io::Result<()> {
    let stream = UnixStream::connect(socket_path()).await?;
    relay(tokio::io::stdin(), tokio::io::stdout(), stream).await
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
