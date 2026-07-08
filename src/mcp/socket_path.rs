//! Deterministic, per-user Unix domain socket endpoint for the MCP server.
//!
//! This is the Linux analogue of the Windows named-pipe scheme in
//! `CanfarDesktop/Mcp/McpPipeName.cs`. On Windows the app (server) and the
//! bridge (client) both run as the same user and independently compute the
//! SAME named-pipe name with no file handoff. We want the same property here:
//! a stable, per-user endpoint that both sides can derive without any prior
//! coordination.
//!
//! The natural Linux equivalent is a Unix domain socket. We prefer
//! `$XDG_RUNTIME_DIR` (a per-user, typically `0700`, tmpfs directory managed
//! by the login session — e.g. `/run/user/<uid>`) because the OS already
//! guarantees it is owned by, and private to, the current user. Security comes
//! from the containing directory's ownership/permissions, not from the socket
//! name being secret — mirroring the C# comment that relies on the owner-only
//! pipe ACL rather than name secrecy.
//!
//! When `XDG_RUNTIME_DIR` is not set (headless sessions, cron, some SSH logins)
//! we fall back to `/tmp/verbinal-mcp-<uid>.sock`. The `<uid>` keeps the path
//! deterministic and unique per user so two users on the same host never
//! collide on the shared, world-writable `/tmp`.

use std::path::PathBuf;

/// The socket filename used inside `$XDG_RUNTIME_DIR`.
const SOCKET_FILE_NAME: &str = "verbinal-mcp.sock";

/// Prefix for the `/tmp` fallback; the current uid is appended, then `.sock`.
const TMP_FALLBACK_PREFIX: &str = "verbinal-mcp-";

/// Returns the deterministic, per-user path to the MCP Unix domain socket.
///
/// - Preferred: `$XDG_RUNTIME_DIR/verbinal-mcp.sock`
/// - Fallback (when `XDG_RUNTIME_DIR` is unset or empty):
///   `/tmp/verbinal-mcp-<uid>.sock`
///
/// The path is stable across restarts and unique per user, so the server and
/// any client can each compute it independently with no handoff.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => {
            let mut path = PathBuf::from(dir);
            path.push(SOCKET_FILE_NAME);
            path
        }
        _ => tmp_fallback_path(current_uid()),
    }
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

    #[test]
    fn prefers_xdg_runtime_dir_when_set() {
        let path = with_env_var("XDG_RUNTIME_DIR", Some("/run/user/4242"), socket_path);
        assert_eq!(path, PathBuf::from("/run/user/4242/verbinal-mcp.sock"));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(SOCKET_FILE_NAME)
        );
    }

    #[test]
    fn falls_back_to_tmp_when_xdg_unset() {
        let path = with_env_var("XDG_RUNTIME_DIR", None, socket_path);
        let expected = tmp_fallback_path(current_uid());
        assert_eq!(path, expected);
        assert_eq!(path.parent(), Some(std::path::Path::new("/tmp")));
        assert!(path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .ends_with(".sock"));
    }

    #[test]
    fn falls_back_to_tmp_when_xdg_empty() {
        let path = with_env_var("XDG_RUNTIME_DIR", Some(""), socket_path);
        assert_eq!(path, tmp_fallback_path(current_uid()));
    }

    #[test]
    fn tmp_fallback_path_is_deterministic_and_uid_scoped() {
        assert_eq!(
            tmp_fallback_path(1000),
            PathBuf::from("/tmp/verbinal-mcp-1000.sock")
        );
        assert_ne!(tmp_fallback_path(1000), tmp_fallback_path(1001));
    }

    /// Runs `f` with `XDG_RUNTIME_DIR` temporarily set (or removed), then
    /// restores the previous value. These tests are serialized via a mutex
    /// because they mutate shared process environment state.
    fn with_env_var<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os(key);

        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }

        let result = f();

        match previous {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }

        result
    }
}
