//! Claude Desktop / Claude Code configuration for the Verbinal MCP server.
//!
//! This is the Linux analogue of `CanfarDesktop/Mcp/Config/*`. Claude Desktop
//! reads a `claude_desktop_config.json` and launches each configured server's
//! `command`+`args` over stdio; our `verbinal mcp` subcommand is exactly that
//! stdio↔socket bridge. Here we:
//!
//! * compute the command line Claude should launch ([`verbinal_command`]),
//! * locate the config file Claude Desktop reads ([`claude_desktop_config_path`]),
//! * MERGE our server entry into it while preserving every sibling server and
//!   all other top-level keys ([`merged_config`] / [`apply_to_claude_desktop`]),
//! * and expose the one-liner a user pastes for Claude Code ([`claude_code_add_command`]).
//!
//! The merge is 1-to-1 with `Mcp/Config/ClaudeConfigMerge.cs`: same
//! `mcpServers[SERVER_KEY]` shape, same "unparseable → start fresh (the .bak
//! keeps the old)" policy, same atomic temp+rename write with a `.bak` backup.

use directories::BaseDirs;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The stable key for our server inside the config's `mcpServers` map.
///
/// Matches `ClaudeConfigMerge.ServerKey` on Windows so a user who moves between
/// platforms keeps the same logical server entry.
pub const SERVER_KEY: &str = "verbinal-canfar";

/// The command line Claude should launch to reach us: the current executable
/// path plus the `mcp` subcommand that enters stdio-bridge mode.
///
/// Falls back to the bare name `verbinal` (resolved on `PATH`) only if the
/// running executable's path can't be determined — a near-impossible case that
/// still yields a launchable command for a normally-installed binary.
pub fn verbinal_command() -> (String, Vec<String>) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "verbinal".to_string());
    (exe, vec!["mcp".to_string()])
}

/// Path to the `claude_desktop_config.json` Claude Desktop reads on Linux:
/// `$XDG_CONFIG_HOME/Claude/claude_desktop_config.json` (i.e. `~/.config/Claude/…`).
///
/// The Windows locator has to chase Store/MSIX package containers
/// (`ClaudeConfigLocator.cs`); on Linux the plain XDG config dir is the single
/// canonical location, so this is deterministic.
pub fn claude_desktop_config_path() -> PathBuf {
    let base = BaseDirs::new()
        .map(|b| b.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("Claude").join("claude_desktop_config.json")
}

/// Return the merged config JSON (pretty-printed).
///
/// `existing` may be `None`/empty/whitespace (a fresh `{}` is created) or an
/// unparseable string (also treated as fresh — the caller's `.bak` preserves the
/// original bytes). Ensures an `mcpServers` object exists, then sets
/// `mcpServers[SERVER_KEY] = { "command": command, "args": args }` while
/// preserving every other server and every other top-level key.
pub fn merged_config(existing: Option<&str>, command: &str, args: &[String]) -> Result<String, String> {
    // Parse the existing document, or start fresh. An unparseable or
    // non-object root collapses to `{}` (the original is kept via the .bak).
    let mut root: Value = match existing {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    };
    if !root.is_object() {
        root = json!({});
    }

    // Scope the mutable borrows so `root` is free to serialize afterwards.
    {
        let obj = root
            .as_object_mut()
            .expect("root is guaranteed to be a JSON object above");

        // Ensure `mcpServers` is an object, replacing any non-object value.
        let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
        if !servers.is_object() {
            *servers = json!({});
        }
        let servers_obj = servers
            .as_object_mut()
            .expect("mcpServers is guaranteed to be a JSON object above");

        servers_obj.insert(
            SERVER_KEY.to_string(),
            json!({ "command": command, "args": args }),
        );
    }

    serde_json::to_string_pretty(&root).map_err(|e| e.to_string())
}

/// Merge our server entry into Claude Desktop's config on disk.
///
/// Reads the existing file (if any), merges via [`merged_config`], backs up any
/// existing config to a sibling `.bak`, then writes the result atomically
/// (temp sibling + rename) so a crash mid-write can never leave a truncated
/// config. Creates the `Claude` directory if it doesn't exist yet.
pub fn apply_to_claude_desktop() -> Result<(), String> {
    let (command, args) = verbinal_command();
    let path = claude_desktop_config_path();

    let existing = std::fs::read_to_string(&path).ok();
    let merged = merged_config(existing.as_deref(), &command, &args)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Preserve the user's prior config verbatim before we overwrite it.
    if path.exists() {
        let backup = path.with_extension("json.bak");
        std::fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }

    // Atomic write: a temp sibling in the same directory, then rename.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, merged.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether our server is already present in Claude Desktop's config
/// (`mcpServers[SERVER_KEY]` exists). Returns `false` if the file is missing,
/// unreadable, or unparseable.
pub fn is_configured() -> bool {
    let path = claude_desktop_config_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    root.get("mcpServers")
        .and_then(|servers| servers.get(SERVER_KEY))
        .is_some()
}

/// The `claude mcp add …` command a user pastes to register us with Claude Code
/// (we never auto-edit `~/.claude.json`). Shape:
/// `claude mcp add verbinal-canfar <exe> mcp`.
pub fn claude_code_add_command() -> String {
    let (command, args) = verbinal_command();
    format!("claude mcp add {SERVER_KEY} {command} {}", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp_args() -> Vec<String> {
        vec!["mcp".to_string()]
    }

    #[test]
    fn merged_config_fresh_creates_server_entry() {
        let out = merged_config(None, "/opt/verbinal/verbinal", &mcp_args()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/opt/verbinal/verbinal");
        assert_eq!(v["mcpServers"][SERVER_KEY]["args"], json!(["mcp"]));
    }

    #[test]
    fn merged_config_empty_and_whitespace_treated_as_fresh() {
        for existing in [Some(""), Some("   \n\t "), None] {
            let out = merged_config(existing, "/bin/verbinal", &mcp_args()).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/bin/verbinal");
        }
    }

    #[test]
    fn merged_config_preserves_existing_servers_and_top_level_keys() {
        let existing = r#"{
            "theme": "dark",
            "mcpServers": {
                "other-server": { "command": "foo", "args": ["bar", "baz"] }
            }
        }"#;
        let out = merged_config(Some(existing), "/usr/bin/verbinal", &mcp_args()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        // Unrelated top-level key survives.
        assert_eq!(v["theme"], "dark");
        // Sibling server and its args survive untouched.
        assert_eq!(v["mcpServers"]["other-server"]["command"], "foo");
        assert_eq!(v["mcpServers"]["other-server"]["args"], json!(["bar", "baz"]));
        // Our entry is added alongside it.
        assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/usr/bin/verbinal");
        assert_eq!(v["mcpServers"][SERVER_KEY]["args"], json!(["mcp"]));
    }

    #[test]
    fn merged_config_overwrites_only_our_own_stale_entry() {
        let existing = format!(
            r#"{{ "mcpServers": {{ "{SERVER_KEY}": {{ "command": "OLD", "args": [] }} }} }}"#
        );
        let out = merged_config(Some(&existing), "/new/path", &mcp_args()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/new/path");
        assert_eq!(v["mcpServers"][SERVER_KEY]["args"], json!(["mcp"]));
    }

    #[test]
    fn merged_config_unparseable_starts_fresh_but_still_writes_us() {
        let out = merged_config(Some("{ this is not json"), "/x", &mcp_args()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/x");
    }

    #[test]
    fn merged_config_non_object_mcpservers_is_replaced() {
        // A malformed `mcpServers` (an array) is discarded, not merged into.
        let out = merged_config(Some(r#"{ "mcpServers": [1, 2, 3] }"#), "/y", &mcp_args()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["mcpServers"].is_object());
        assert_eq!(v["mcpServers"][SERVER_KEY]["command"], "/y");
    }

    #[test]
    fn config_path_ends_in_expected_location() {
        let path = claude_desktop_config_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("claude_desktop_config.json")
        );
        assert_eq!(
            path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
            Some("Claude")
        );
    }

    #[test]
    fn claude_code_add_command_has_expected_shape() {
        let cmd = claude_code_add_command();
        assert!(cmd.starts_with(&format!("claude mcp add {SERVER_KEY} ")));
        assert!(cmd.ends_with(" mcp"), "should pass the `mcp` subcommand: {cmd}");
    }

    #[test]
    fn verbinal_command_uses_mcp_subcommand() {
        let (_exe, args) = verbinal_command();
        assert_eq!(args, vec!["mcp".to_string()]);
    }
}
