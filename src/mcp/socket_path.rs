//! Deterministic, per-user Unix domain socket endpoint for the MCP server.
//!
//! This is the Linux analogue of the Windows named-pipe scheme in
//! `CanfarDesktop/Mcp/McpPipeName.cs`. On Windows the app (server) and the
//! bridge (client) both run as the same user and independently compute the
//! SAME named-pipe name with no file handoff. We want the same property here:
//! a stable, per-user endpoint that both sides can derive without any prior
//! coordination.
//!
//! The natural Linux equivalent is a Unix domain socket, placed in the per-user
//! runtime directory (`/run/user/<uid>` — a `0700`, tmpfs dir managed by the
//! login session). Security comes from that directory's ownership/permissions,
//! not from the socket name being secret — mirroring the C# comment that relies
//! on the owner-only pipe ACL rather than name secrecy.
//!
//! ## Why NOT trust `$XDG_RUNTIME_DIR` for the path
//!
//! The two sides do **not** share an environment. The app (server) runs in the
//! full desktop session, but the bridge (`verbinal mcp`) is spawned by the MCP
//! client — and Claude Desktop launches it through the MCP SDK's
//! `getDefaultEnvironment()`, which passes only an allowlist
//! (`HOME, LOGNAME, PATH, SHELL, TERM, USER`) and **strips `XDG_RUNTIME_DIR`**.
//! If the path were `$XDG_RUNTIME_DIR/…`, the server would bind
//! `/run/user/<uid>/verbinal-mcp.sock` while the bridge — seeing no
//! `XDG_RUNTIME_DIR` — would look somewhere else entirely and never connect
//! (the bridge just times out and Claude Desktop reports "Connection closed").
//!
//! So we derive the runtime directory from the **uid** instead: `/run/user/<uid>`
//! is the standard systemd location and, crucially, `getuid()` returns the same
//! value on both sides regardless of what the launcher did to the environment.
//! `$XDG_RUNTIME_DIR` is consulted only as a fallback for the rare host where
//! `/run/user/<uid>` doesn't exist (non-systemd, some containers), and
//! `/tmp/verbinal-mcp-<uid>.sock` as the last resort. The `<uid>` there keeps the
//! path unique per user on the shared, world-writable `/tmp`.
//!
//! Because the uid-derived path deliberately outranks `$XDG_RUNTIME_DIR`, moving
//! that variable is NOT enough to isolate a second instance — an explicit
//! [`SOCKET_OVERRIDE_VAR`] names the socket outright and beats every rule above.

use std::path::PathBuf;

/// The socket filename inside the per-user runtime directory.
const SOCKET_FILE_NAME: &str = "verbinal-mcp.sock";

/// Prefix for the `/tmp` fallback; the current uid is appended, then `.sock`.
const TMP_FALLBACK_PREFIX: &str = "verbinal-mcp-";

/// Environment variable naming an explicit socket path, overriding every other
/// rule. Set it on BOTH the app and the bridge to point them at the same private
/// socket — used by the integration tests for isolation, and by anyone running a
/// second instance without disturbing the primary one.
pub const SOCKET_OVERRIDE_VAR: &str = "VERBINAL_MCP_SOCKET";

/// Returns the deterministic, per-user path to the MCP Unix domain socket.
///
/// Resolution order (see [`resolve_socket_path`] for the pure logic):
/// 0. `$VERBINAL_MCP_SOCKET` verbatim, when set and non-empty — an explicit
///    opt-in escape hatch for tests and side-by-side instances.
/// 1. `/run/user/<uid>/verbinal-mcp.sock` when `/run/user/<uid>` exists — derived
///    from the uid, so the server and the (env-stripped) bridge agree without any
///    shared environment.
/// 2. `$XDG_RUNTIME_DIR/verbinal-mcp.sock` when set and `/run/user/<uid>` is absent.
/// 3. `/tmp/verbinal-mcp-<uid>.sock` as a last resort.
///
/// The path is stable across restarts and unique per user, so the server and any
/// client can each compute it independently with no handoff.
pub fn socket_path() -> PathBuf {
    let uid = current_uid();
    let run_user_exists = PathBuf::from(format!("/run/user/{uid}")).is_dir();
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    let override_path = std::env::var(SOCKET_OVERRIDE_VAR).ok();
    resolve_socket_path_with_override(
        override_path.as_deref(),
        xdg.as_deref(),
        uid,
        run_user_exists,
    )
}

/// [`resolve_socket_path`] preceded by the explicit-override check.
fn resolve_socket_path_with_override(
    override_path: Option<&str>,
    xdg: Option<&str>,
    uid: u32,
    run_user_exists: bool,
) -> PathBuf {
    if let Some(p) = override_path {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    resolve_socket_path(xdg, uid, run_user_exists)
}

/// Pure path resolution, factored out so the (environment- and
/// filesystem-dependent) decision is unit-testable with controlled inputs.
///
/// `run_user_exists` is whether `/run/user/<uid>` is a directory; `xdg` is the
/// raw `$XDG_RUNTIME_DIR` value (`None` when unset). See [`socket_path`] for the
/// resolution order and the rationale for preferring the uid-derived path.
fn resolve_socket_path(xdg: Option<&str>, uid: u32, run_user_exists: bool) -> PathBuf {
    // 1. Env-free canonical runtime dir: both the server (full env) and the
    //    bridge (env stripped of XDG_RUNTIME_DIR by the MCP launcher) derive the
    //    SAME path from the uid, so they always meet.
    if run_user_exists {
        return PathBuf::from(format!("/run/user/{uid}")).join(SOCKET_FILE_NAME);
    }
    // 2. A runtime dir explicitly provided via the environment, for hosts without
    //    a systemd `/run/user/<uid>`. (Both sides can only agree here if the
    //    launcher preserved the var; the /tmp step below is the shared fallback.)
    if let Some(dir) = xdg {
        if !dir.is_empty() {
            return PathBuf::from(dir).join(SOCKET_FILE_NAME);
        }
    }
    // 3. Last resort on systems with neither `/run/user/<uid>` nor XDG.
    tmp_fallback_path(uid)
}

/// Builds the `/tmp/verbinal-mcp-<uid>.sock` fallback path for a given uid.
fn tmp_fallback_path(uid: u32) -> PathBuf {
    PathBuf::from("/tmp").join(format!("{TMP_FALLBACK_PREFIX}{uid}.sock"))
}

/// The current process's real user id.
///
/// `getuid` is always successful and carries no error contract, so the `unsafe`
/// FFI call is sound: it merely reads a value the kernel maintains for the
/// process.
fn current_uid() -> u32 {
    // SAFETY: getuid() has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_ends_with_expected_filename() {
        // Regardless of which branch produced it, the path must end in a
        // `.sock` file whose name identifies this app.
        let path = socket_path();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("socket path must have a UTF-8 file name");

        assert!(
            file_name == SOCKET_FILE_NAME || file_name.starts_with(TMP_FALLBACK_PREFIX),
            "unexpected socket file name: {file_name}"
        );
        assert!(
            file_name.ends_with(".sock"),
            "socket file name must end with .sock: {file_name}"
        );
    }

    /// THE regression guard for the "Claude Desktop can't connect" bug: with the
    /// environment stripped of `XDG_RUNTIME_DIR` (exactly how the MCP launcher
    /// spawns the bridge), the path must still resolve to the uid's `/run/user`
    /// socket — the same one the server (full env) binds — so the two meet.
    #[test]
    fn resolves_to_run_user_without_xdg_when_it_exists() {
        assert_eq!(
            resolve_socket_path(None, 1000, true),
            PathBuf::from("/run/user/1000/verbinal-mcp.sock")
        );
    }

    /// The server (with `XDG_RUNTIME_DIR` set) and the bridge (with it stripped)
    /// must derive the IDENTICAL path whenever `/run/user/<uid>` exists — the
    /// core invariant that makes a handoff-free connection possible.
    #[test]
    fn server_and_bridge_agree_regardless_of_xdg() {
        let server = resolve_socket_path(Some("/run/user/1000"), 1000, true);
        let bridge = resolve_socket_path(None, 1000, true);
        assert_eq!(server, bridge);
        assert_eq!(server, PathBuf::from("/run/user/1000/verbinal-mcp.sock"));
    }

    /// The uid-derived path wins even over a non-standard `$XDG_RUNTIME_DIR`, so
    /// both sides still agree (the bridge can't see that custom value anyway).
    #[test]
    fn run_user_takes_precedence_over_custom_xdg() {
        assert_eq!(
            resolve_socket_path(Some("/some/custom/dir"), 1000, true),
            PathBuf::from("/run/user/1000/verbinal-mcp.sock")
        );
    }

    /// The explicit override beats every other rule — including the uid-derived
    /// path that otherwise wins. This is the ONLY way to isolate a second
    /// instance (or an integration test) from the user's live app socket.
    #[test]
    fn explicit_override_beats_run_user_and_xdg() {
        assert_eq!(
            resolve_socket_path_with_override(
                Some("/tmp/private-test.sock"),
                Some("/some/custom/dir"),
                1000,
                true
            ),
            PathBuf::from("/tmp/private-test.sock")
        );
    }

    /// An unset or empty override falls through to the normal resolution order,
    /// so a stray empty env var can never redirect the socket to `""`.
    #[test]
    fn empty_or_absent_override_falls_through() {
        let expected = PathBuf::from("/run/user/1000/verbinal-mcp.sock");
        assert_eq!(
            resolve_socket_path_with_override(None, None, 1000, true),
            expected
        );
        assert_eq!(
            resolve_socket_path_with_override(Some(""), None, 1000, true),
            expected
        );
    }

    /// On a host without `/run/user/<uid>`, an explicit `$XDG_RUNTIME_DIR` is used.
    #[test]
    fn uses_xdg_when_run_user_absent() {
        assert_eq!(
            resolve_socket_path(Some("/run/user/4242"), 4242, false),
            PathBuf::from("/run/user/4242/verbinal-mcp.sock")
        );
    }

    /// With neither `/run/user/<uid>` nor a usable `$XDG_RUNTIME_DIR`, fall back
    /// to the uid-scoped `/tmp` path (unset and empty both count as unusable).
    #[test]
    fn falls_back_to_tmp_without_run_user_or_xdg() {
        assert_eq!(
            resolve_socket_path(None, 1000, false),
            tmp_fallback_path(1000)
        );
        assert_eq!(
            resolve_socket_path(Some(""), 1000, false),
            tmp_fallback_path(1000)
        );
        assert_eq!(
            resolve_socket_path(None, 1000, false).parent(),
            Some(std::path::Path::new("/tmp"))
        );
    }

    #[test]
    fn tmp_fallback_path_is_deterministic_and_uid_scoped() {
        assert_eq!(
            tmp_fallback_path(1000),
            PathBuf::from("/tmp/verbinal-mcp-1000.sock")
        );
        assert_ne!(tmp_fallback_path(1000), tmp_fallback_path(1001));
    }
}
