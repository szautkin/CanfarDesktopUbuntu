//! Provenance stamp for entities created or modified by an AI agent over MCP.
//!
//! Ported from `Models/AgentAttribution.cs` + `Mcp/Agents/AgentAttribution.cs`.
//! Attached (optionally) to saved queries, observation notes, etc. so the UI can
//! show a "created by an agent" badge with details.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAttribution {
    /// Display label of the agent/client (e.g. "Claude Desktop").
    pub client: String,
    /// The MCP tool that performed the action (e.g. "save_query").
    pub tool: String,
    /// ISO-8601 timestamp of the action.
    pub timestamp: String,
    /// Short stable fingerprint = first 6 hex of SHA-256(client label).
    pub fingerprint: String,
}

impl AgentAttribution {
    pub fn new(client: impl Into<String>, tool: impl Into<String>, timestamp: impl Into<String>) -> Self {
        let client = client.into();
        let fingerprint = fingerprint(&client);
        AgentAttribution {
            client,
            tool: tool.into(),
            timestamp: timestamp.into(),
            fingerprint,
        }
    }
}

/// First 6 hex chars of SHA-256(label) — a stable short id for the agent.
pub fn fingerprint(label: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(label.as_bytes());
    hex::encode(digest)[..6].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_6_hex() {
        let f = fingerprint("Claude Desktop");
        assert_eq!(f.len(), 6);
        assert_eq!(f, fingerprint("Claude Desktop"));
        assert!(f.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
