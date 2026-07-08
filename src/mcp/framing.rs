//! NDJSON frame codec for the MCP transports.
//!
//! Wire format: **one JSON document per line**, terminated by a single `\n`
//! (stdio / Claude Desktop / Claude Code CLI). A trailing `\r` before the `\n`
//! is tolerated so a peer that emits CRLF still decodes cleanly. This is the
//! async, streaming equivalent of the reference C#
//! `CanfarDesktop.Mcp.Transport.FrameCodec` (NDJSON mode): the same frame-size
//! limit ([`constants::MAX_FRAME_BYTES`]) and the same CRLF tolerance.
//!
//! [`read_frame`] pulls one complete frame from any [`AsyncBufRead`] and
//! [`write_frame`] appends a single `\n` and flushes. Frames carry no embedded
//! newlines because a serialized `serde_json` document is single-line, so one
//! line is always exactly one JSON-RPC message.

use crate::mcp::constants::MAX_FRAME_BYTES;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Read one `\n`-delimited frame from `r`.
///
/// * Returns `Ok(None)` at end-of-stream (no more bytes at all).
/// * Returns `Ok(Some(bytes))` for a complete line, with the trailing `\n` and
///   an optional preceding `\r` stripped. An empty line yields
///   `Ok(Some(vec![]))` — a keep-alive that the caller should ignore.
/// * Returns `Err(InvalidData)` if the frame content would exceed
///   [`constants::MAX_FRAME_BYTES`]. The limit is enforced incrementally as the
///   line is reassembled, so an unterminated runaway document is rejected before
///   it is fully buffered.
///
/// If the stream ends with trailing bytes that were never `\n`-terminated, those
/// bytes are returned as the final frame; the *next* call then reports EOF. This
/// matches stdio peers whose last message may omit the terminator.
pub async fn read_frame<R>(r: &mut R) -> std::io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    let mut saw_newline = false;

    loop {
        let chunk = r.fill_buf().await?;

        if chunk.is_empty() {
            // EOF. Nothing buffered => clean end of stream.
            if buf.is_empty() {
                return Ok(None);
            }
            // Trailing, unterminated bytes => hand them back as a final frame.
            break;
        }

        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                buf.extend_from_slice(&chunk[..i]);
                r.consume(i + 1);
                saw_newline = true;
                break;
            }
            None => {
                let n = chunk.len();
                buf.extend_from_slice(chunk);
                r.consume(n);
                // Guard while still accumulating: reject a runaway line early.
                if buf.len() > MAX_FRAME_BYTES {
                    return Err(frame_too_large(buf.len()));
                }
            }
        }
    }

    // Tolerate CRLF: a `\n`-terminated line may leave a trailing `\r`.
    if saw_newline && buf.last() == Some(&b'\r') {
        buf.pop();
    }

    if buf.len() > MAX_FRAME_BYTES {
        return Err(frame_too_large(buf.len()));
    }

    Ok(Some(buf))
}

/// Write `frame` as one NDJSON line: the raw bytes followed by a single `\n`,
/// then flush. `frame` must be a single-line JSON document (no embedded `\n`);
/// `serde_json::to_vec` already produces exactly that.
pub async fn write_frame<W>(w: &mut W, frame: &[u8]) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    w.write_all(frame).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

fn frame_too_large(len: usize) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("frame too large ({len} > {MAX_FRAME_BYTES})"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// Wrap a byte slice as an `AsyncBufRead` for the reader under test.
    fn reader(bytes: &[u8]) -> BufReader<&[u8]> {
        BufReader::new(bytes)
    }

    #[tokio::test]
    async fn round_trip_single_document() {
        let doc = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;

        let mut sink: Vec<u8> = Vec::new();
        write_frame(&mut sink, doc).await.unwrap();

        // Exactly the document plus one newline, no embedded newlines.
        assert_eq!(sink.last(), Some(&b'\n'));
        assert_eq!(&sink[..sink.len() - 1], &doc[..]);
        assert_eq!(sink.iter().filter(|&&b| b == b'\n').count(), 1);

        let mut r = reader(&sink);
        let frame = read_frame(&mut r).await.unwrap().unwrap();
        assert_eq!(frame, doc);

        // Stream is now drained.
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn eof_on_empty_stream() {
        let mut r = reader(b"");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reads_multiple_frames_in_order() {
        let mut r = reader(b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"b\":2}");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn strips_trailing_cr_for_crlf_peer() {
        let mut r = reader(b"{\"x\":true}\r\n");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"x\":true}");
    }

    #[tokio::test]
    async fn empty_line_is_keep_alive() {
        let mut r = reader(b"\n{\"y\":9}\n");
        // Blank line => empty (but Some) frame the caller ignores.
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"y\":9}");
    }

    #[tokio::test]
    async fn unterminated_tail_is_final_frame() {
        // No trailing newline on the last message.
        let mut r = reader(b"{\"z\":0}");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"z\":0}");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversize_frame_is_rejected() {
        // One byte past the limit, with a terminator, must still error.
        let mut data = vec![b'a'; MAX_FRAME_BYTES + 1];
        data.push(b'\n');
        let mut r = reader(&data);
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn exactly_max_bytes_is_accepted() {
        let mut data = vec![b'a'; MAX_FRAME_BYTES];
        data.push(b'\n');
        let mut r = reader(&data);
        let frame = read_frame(&mut r).await.unwrap().unwrap();
        assert_eq!(frame.len(), MAX_FRAME_BYTES);
    }

    #[tokio::test]
    async fn round_trip_over_duplex_pipe() {
        let (a, b) = tokio::io::duplex(64);
        let doc = br#"{"jsonrpc":"2.0","result":{}}"#;

        let writer = tokio::spawn(async move {
            let mut w = a;
            write_frame(&mut w, doc).await.unwrap();
        });

        let mut rd = BufReader::new(b);
        let frame = read_frame(&mut rd).await.unwrap().unwrap();
        assert_eq!(frame, doc);

        writer.await.unwrap();
    }
}
