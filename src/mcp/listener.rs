//! In-app UNIX-domain-socket listener (the Linux equivalent of the Windows
//! named-pipe `McpListenerService`).
//!
//! The running app binds a per-user socket in the uid's runtime directory (see
//! [`crate::mcp::socket_path`]), hardens it to owner-only, then accepts
//! connections forever. Each accepted connection is served concurrently by a
//! fresh [`crate::mcp::server::serve`] task — one server instance per
//! connection, per the MCP contract — so a client can connect while another is
//! still being served. A single connection failing (client hung up, protocol
//! error) must never take down the accept loop.
//!
//! 1-to-1 with `Mcp/Listener/McpListenerService.cs`.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::UnixListener;

use crate::mcp::server::{self, ApprovalGate};
use crate::mcp::socket_path::socket_path;
use crate::mcp::tools::ToolRouter;

/// Bind the control socket and serve connections until an accept error occurs.
///
/// Steps, mirroring the reference listener:
/// 1. Remove any stale socket file left by a previous (crashed) run — otherwise
///    `bind` fails with `EADDRINUSE`.
/// 2. Bind a [`UnixListener`] on [`socket_path()`].
/// 3. Restrict the socket to the owner (mode `0o700`) so no other local user
///    can drive the app.
/// 4. Accept forever, spawning [`server::serve`] per connection.
///
/// Returns only when [`UnixListener::accept`] itself errors (the loop is
/// otherwise infinite); per-connection errors are swallowed inside the spawned
/// task and never propagate here.
pub async fn run(router: Arc<dyn ToolRouter>, gate: Arc<dyn ApprovalGate>) -> io::Result<()> {
    run_on_path(socket_path(), router, gate).await
}

/// [`run`] against an explicit socket `path`.
///
/// Split out so tests can drive the full bind → harden → accept → serve path
/// over a private temp socket without depending on the process-wide
/// `XDG_RUNTIME_DIR`-derived endpoint (which a live app instance may own).
/// Production always reaches this through [`run`] with [`socket_path()`].
pub(crate) async fn run_on_path(
    path: PathBuf,
    router: Arc<dyn ToolRouter>,
    gate: Arc<dyn ApprovalGate>,
) -> io::Result<()> {
    // A leftover socket node from a previous run makes bind() fail; clear it.
    // A missing file is the normal case, so ignore the error.
    let _ = std::fs::remove_file(&path);

    // Best-effort: the socket may live in a per-app subdirectory of the runtime
    // dir that does not exist yet on a fresh login.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = UnixListener::bind(&path)?;

    // Owner-only: no other user on the machine may connect to the control
    // socket. Done after bind so the node exists.
    harden(&path)?;

    loop {
        let (stream, _addr) = listener.accept().await?;
        let router = router.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            // One server per connection. Whatever `serve` returns (Ok, an I/O
            // error, or a protocol close), we drop it — the accept loop lives on.
            let _ = server::serve(stream, router, gate).await;
        });
    }
}

/// Restrict `path` to the owner (`rwx------`, mode `0o700`).
fn harden(path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harden_makes_socket_owner_only() {
        // Use std's UnixListener here so the test needs no tokio runtime and no
        // dependency on the (separately generated) socket_path/server modules.
        let dir = std::env::temp_dir().join(format!(
            "verbinal-listener-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.sock");
        let _ = std::fs::remove_file(&path);

        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        harden(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "socket must be owner-only");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
