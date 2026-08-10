//! MCP integration diagnostics — a battery of fast, local, read-only checks the
//! Settings "Run diagnostics" button surfaces so a user can tell at a glance why
//! an external AI agent can (or can't) reach Verbinal.
//!
//! Linux port of `Mcp/McpDiagnostics.cs`. The Windows runner mixes a live named-
//! pipe self-test into the same list; here we keep the core purely to the cheap
//! local probes (the wire round-trip lives in [`crate::mcp::selftest`]). As in the
//! C# original, every row's *decision* is a pure function of already-gathered
//! facts — the impure probing (env, filesystem, `is_running`) happens once in
//! [`run_diagnostics`] and the row builders below are unit-testable without the
//! app, a socket, or the real filesystem.
//!
//! The checks mirror the Windows rows, adapted to the Linux transport:
//! 1. MCP server running (`services.mcp_host.is_running()`),
//! 2. the per-user control socket is present / creatable (see
//!    [`crate::mcp::socket_path`]),
//! 3. the `verbinal mcp` stdio bridge argv is available (the running exe exists),
//! 4. Claude Desktop's config exists and registers Verbinal
//!    (`~/.config/Claude/claude_desktop_config.json`),
//! 5. Claude Code CLI is present (`claude` on `PATH` or `~/.claude`).
//!
//! Every failing row carries a `fix_hint` describing the one concrete next step.

use std::path::Path;

use crate::state::AppServices;

/// One diagnostic result row. `ok` is the pass/fail verdict; `fix_hint` is
/// populated with the single concrete remedy exactly when `ok` is `false`.
pub struct DiagRow {
    pub label: String,
    pub ok: bool,
    pub detail: String,
    pub fix_hint: Option<String>,
}

impl DiagRow {
    /// A passing row (no fix hint).
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        DiagRow {
            label: label.into(),
            ok: true,
            detail: detail.into(),
            fix_hint: None,
        }
    }

    /// A failing row, always paired with the remedy to show the user.
    fn fail(
        label: impl Into<String>,
        detail: impl Into<String>,
        fix_hint: impl Into<String>,
    ) -> Self {
        DiagRow {
            label: label.into(),
            ok: false,
            detail: detail.into(),
            fix_hint: Some(fix_hint.into()),
        }
    }
}

/// Run the MCP diagnostics against the live services and return one row per
/// check, in a stable order. Synchronous and side-effect-free (read-only
/// probes): safe to call directly from a UI button handler.
pub fn run_diagnostics(services: &AppServices) -> Vec<DiagRow> {
    // ── Gather the impure facts once, up front ──
    let socket = crate::mcp::socket_path::socket_path();
    let socket_parent_writable = socket.parent().map(dir_writable).unwrap_or(false);

    let (bridge_command, bridge_exe_exists) = probe_bridge();

    let config_path = crate::mcp::config::claude_desktop_config_path();
    let config_exists = config_path.exists();
    let config_registers = crate::mcp::config::is_configured();

    // ── Delegate every verdict to a pure builder ──
    vec![
        server_running_row(services.mcp_host.is_running()),
        control_socket_row(&socket, socket_parent_writable),
        bridge_argv_row(&bridge_command, bridge_exe_exists),
        claude_desktop_config_row(&config_path, config_exists, config_registers),
        claude_code_row(claude_on_path(), claude_home_dir_exists()),
    ]
}

// ─────────────────────────── pure row builders ────────────────────────────

/// Row 1 — is the app-hosted MCP listener live?
fn server_running_row(running: bool) -> DiagRow {
    if running {
        DiagRow::pass(
            "MCP server",
            "Running and accepting external AI-agent connections.",
        )
    } else {
        DiagRow::fail(
            "MCP server",
            "Not running.",
            "Enable the MCP server in Settings so external AI agents (Claude Desktop / Claude Code) can connect.",
        )
    }
}

/// Row 2 — can the per-user control socket be created at its endpoint? The
/// socket file itself only exists while the listener is bound, so the useful
/// check is that its containing directory exists and is writable by us.
fn control_socket_row(socket_path: &Path, parent_writable: bool) -> DiagRow {
    let shown = socket_path.display();
    if parent_writable {
        DiagRow::pass("Control socket", format!("Endpoint ready at {shown}"))
    } else {
        DiagRow::fail(
            "Control socket",
            format!(
                "Can't create the socket at {shown} — its directory is missing or not writable."
            ),
            "Ensure $XDG_RUNTIME_DIR (or /tmp) exists and is writable by your user.",
        )
    }
}

/// Row 3 — is the `verbinal mcp` stdio bridge argv actually launchable, i.e.
/// does the executable Claude would spawn exist on disk?
fn bridge_argv_row(command_line: &str, exe_exists: bool) -> DiagRow {
    if exe_exists {
        DiagRow::pass("Bridge command", format!("Claude launches: {command_line}"))
    } else {
        DiagRow::fail(
            "Bridge command",
            format!("The executable for `{command_line}` was not found on disk."),
            "Reinstall Verbinal so its binary exists; `verbinal mcp` is the stdio bridge Claude launches.",
        )
    }
}

/// Row 4 — does Claude Desktop's config exist and register Verbinal? Note
/// `registers` can only be true when the file exists, so `(true, true)` and
/// `(true, false)` and `(false, false)` are the reachable cases.
fn claude_desktop_config_row(config_path: &Path, exists: bool, registers: bool) -> DiagRow {
    let shown = config_path.display();
    match (exists, registers) {
        (_, true) => DiagRow::pass(
            "Claude Desktop config",
            format!("{shown} registers the Verbinal MCP server."),
        ),
        (true, false) => DiagRow::fail(
            "Claude Desktop config",
            format!("{shown} exists but has no Verbinal entry."),
            "Run \"Connect Claude Desktop\" to add the Verbinal MCP server to the config.",
        ),
        (false, false) => DiagRow::fail(
            "Claude Desktop config",
            format!("No config found at {shown}."),
            "Install Claude Desktop, then run \"Connect Claude Desktop\" to register Verbinal (optional — other MCP clients still work).",
        ),
    }
}

/// Row 5 — is Claude Code installed? A `claude` launcher on `PATH` is the
/// strong signal; a `~/.claude` directory is the fallback signal.
fn claude_code_row(on_path: bool, home_dir_exists: bool) -> DiagRow {
    match (on_path, home_dir_exists) {
        (true, _) => DiagRow::pass("Claude Code CLI", "The `claude` CLI is on your PATH."),
        (false, true) => DiagRow::pass(
            "Claude Code CLI",
            "Found ~/.claude — Claude Code appears to be installed.",
        ),
        (false, false) => DiagRow::fail(
            "Claude Code CLI",
            "No `claude` launcher on PATH and no ~/.claude directory.",
            "Install Claude Code, then run its `claude mcp add` command (optional — other MCP clients still work).",
        ),
    }
}

// ─────────────────────────────── impure probes ────────────────────────────

/// The `verbinal mcp` command Claude would launch, plus whether the running
/// executable actually exists on disk. When the current exe path can't be
/// resolved we report "not found" — the safe, actionable verdict.
fn probe_bridge() -> (String, bool) {
    let (command, args) = crate::mcp::config::verbinal_command();
    let exe_exists = std::env::current_exe().map(|p| p.exists()).unwrap_or(false);
    let command_line = if args.is_empty() {
        command
    } else {
        format!("{command} {}", args.join(" "))
    };
    (command_line, exe_exists)
}

/// Whether `dir` exists, is a directory, and is writable by this process — so a
/// socket (or any file) could be created inside it.
fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if !dir.is_dir() {
        return false;
    }
    match std::ffi::CString::new(dir.as_os_str().as_bytes()) {
        // SAFETY: `access` only reads the path and the kernel's permission bits;
        // the CString is a valid, NUL-terminated pointer for the call's duration.
        Ok(c) => unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 },
        // A path containing an interior NUL can't name a real directory.
        Err(_) => false,
    }
}

/// Is a `claude` launcher present in any `PATH` directory?
fn claude_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|dir| !dir.as_os_str().is_empty() && dir.join("claude").is_file())
}

/// Does `~/.claude` exist? (Claude Code's per-user state directory.)
fn claude_home_dir_exists() -> bool {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".claude").exists())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every failing row must carry a fix hint; every passing row must not —
    /// the UI relies on `fix_hint.is_some() == !ok`.
    fn assert_hint_invariant(row: &DiagRow) {
        assert_eq!(
            row.ok,
            row.fix_hint.is_none(),
            "row {:?}: ok/fix_hint invariant violated",
            row.label
        );
    }

    #[test]
    fn server_running_row_reflects_state() {
        let up = server_running_row(true);
        assert!(up.ok);
        assert_hint_invariant(&up);

        let down = server_running_row(false);
        assert!(!down.ok);
        assert!(down.fix_hint.as_deref().unwrap().contains("Settings"));
        assert_hint_invariant(&down);
    }

    #[test]
    fn control_socket_row_creatable_vs_not() {
        let path = Path::new("/run/user/1000/verbinal-mcp.sock");

        let ok = control_socket_row(path, true);
        assert!(ok.ok);
        assert!(ok.detail.contains("verbinal-mcp.sock"));
        assert_hint_invariant(&ok);

        let bad = control_socket_row(path, false);
        assert!(!bad.ok);
        assert!(bad.detail.contains("verbinal-mcp.sock"));
        assert_hint_invariant(&bad);
    }

    #[test]
    fn bridge_argv_row_needs_existing_exe() {
        let present = bridge_argv_row("/opt/verbinal/verbinal mcp", true);
        assert!(present.ok);
        assert!(present.detail.contains("verbinal mcp"));
        assert_hint_invariant(&present);

        let missing = bridge_argv_row("/opt/verbinal/verbinal mcp", false);
        assert!(!missing.ok);
        assert_hint_invariant(&missing);
    }

    #[test]
    fn claude_desktop_config_row_three_cases() {
        let path = Path::new("/home/u/.config/Claude/claude_desktop_config.json");

        let registered = claude_desktop_config_row(path, true, true);
        assert!(registered.ok);
        assert_hint_invariant(&registered);

        let unregistered = claude_desktop_config_row(path, true, false);
        assert!(!unregistered.ok);
        assert!(unregistered.detail.contains("no Verbinal entry"));
        assert_hint_invariant(&unregistered);

        let absent = claude_desktop_config_row(path, false, false);
        assert!(!absent.ok);
        assert!(absent.detail.contains("No config"));
        assert_hint_invariant(&absent);
    }

    #[test]
    fn claude_code_row_three_cases() {
        let on_path = claude_code_row(true, false);
        assert!(on_path.ok);
        assert!(on_path.detail.contains("PATH"));
        assert_hint_invariant(&on_path);

        let via_home = claude_code_row(false, true);
        assert!(via_home.ok);
        assert!(via_home.detail.contains(".claude"));
        assert_hint_invariant(&via_home);

        let neither = claude_code_row(false, false);
        assert!(!neither.ok);
        assert_hint_invariant(&neither);
    }

    #[test]
    fn dir_writable_true_for_tmp_false_for_missing() {
        // /tmp exists and is writable in any sane test environment.
        assert!(dir_writable(Path::new("/tmp")));
        // A path that does not exist is not a writable directory.
        assert!(!dir_writable(Path::new(
            "/nonexistent-verbinal-diagnostics-dir-xyz"
        )));
        // A regular file is not a directory.
        assert!(!dir_writable(Path::new("/etc/hostname")) || !Path::new("/etc/hostname").is_dir());
    }
}
