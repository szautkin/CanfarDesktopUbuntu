//! Provenance stamp left on any persistent entity that originated from an
//! MCP-connected AI agent.
//!
//! Ported from `Models/AgentAttribution.cs` (the record carried inline on
//! `SavedQuery` / `ObservationNote` / `DownloadedObservation`) and
//! `Mcp/Agents/AgentAttribution.cs` (`AgentAttributionStamp.ForProposal`).
//!
//! `None` on the host record means the user authored the entity directly; a
//! `Some(..)` stamp means an external agent proposed the change and the UI should
//! show a small wand badge. It carries only the compact audit metadata (client
//! label, proposal id, summary, applied-at) — never payloads. The short
//! fingerprint is derived on demand from the origin label so it stays 1-to-1 with
//! the badge shown by `models::agent_attribution` / the audit log.

use crate::mcp::tools::proposals::PendingProposal;
use serde::{Deserialize, Serialize};

/// Compact provenance recorded when an agent-originated proposal is applied.
///
/// Field mapping to the reference `Models/AgentAttribution.cs` record:
/// `origin` ↔ `OriginLabel`, `proposal_id` ↔ `ProposalId`,
/// `summary` ↔ `Summary`, `applied_at` ↔ `AppliedAt`. `OriginFingerprint` is not
/// stored — it is a pure function of the label, exposed via [`Self::fingerprint`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentAttribution {
    /// The originating client label (the agent's `origin`, e.g. "Claude Desktop").
    pub origin: String,
    /// The id of the applied proposal that created/edited the record.
    pub proposal_id: String,
    /// The proposal's human-readable one-line summary.
    pub summary: String,
    /// RFC-3339 timestamp of when the proposal was applied.
    pub applied_at: String,
}

impl AgentAttribution {
    /// Build the stamp for an agent-originated proposal at apply time.
    ///
    /// Mirrors `AgentAttributionStamp.ForProposal`. Callers guard on
    /// `proposal.origin.is_some()` (a user-originated proposal gets no badge); if
    /// invoked on a user proposal the origin label degrades to the empty string.
    pub fn for_proposal(proposal: &PendingProposal, now_rfc3339: String) -> Self {
        AgentAttribution {
            origin: proposal.origin.clone().unwrap_or_default(),
            proposal_id: proposal.id.clone(),
            summary: proposal.summary.clone(),
            applied_at: now_rfc3339,
        }
    }

    /// Short stable fingerprint (first 6 hex of SHA-256(origin label)), matching
    /// the badge fingerprint in `models::agent_attribution` and the C#
    /// `AgentActivityEntry.Fingerprint`.
    pub fn fingerprint(&self) -> String {
        crate::models::agent_attribution::fingerprint(&self.origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::proposals::{InMemoryProposalStore, ProposalState};
    use serde_json::json;

    fn proposal_with_origin(origin: Option<&str>) -> PendingProposal {
        let store = InMemoryProposalStore::new();
        let p = store.enqueue(
            "save_query",
            "Save query: M31",
            false,
            json!({"name": "M31"}),
        );
        store.set_origin(&p.id, origin.map(|s| s.to_string()));
        store.get(&p.id).unwrap()
    }

    #[test]
    fn for_proposal_copies_origin_id_and_summary() {
        let p = proposal_with_origin(Some("Claude Desktop"));
        let attr = AgentAttribution::for_proposal(&p, "2026-07-08T00:00:00Z".to_string());
        assert_eq!(attr.origin, "Claude Desktop");
        assert_eq!(attr.proposal_id, p.id);
        assert_eq!(attr.summary, "Save query: M31");
        assert_eq!(attr.applied_at, "2026-07-08T00:00:00Z");
    }

    #[test]
    fn fingerprint_is_six_hex_derived_from_origin() {
        let p = proposal_with_origin(Some("Claude Desktop"));
        let attr = AgentAttribution::for_proposal(&p, "t".to_string());
        let fp = attr.fingerprint();
        assert_eq!(fp.len(), 6);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            fp,
            crate::models::agent_attribution::fingerprint("Claude Desktop")
        );
    }

    #[test]
    fn for_proposal_on_user_origin_yields_empty_label() {
        // Callers guard on origin.is_some(); a defensive call on a user proposal
        // degrades the label to "" rather than panicking.
        let p = proposal_with_origin(None);
        let attr = AgentAttribution::for_proposal(&p, "t".to_string());
        assert_eq!(attr.origin, "");
    }

    #[test]
    fn round_trips_through_json() {
        let attr = AgentAttribution {
            origin: "Claude Desktop".to_string(),
            proposal_id: "prop-7".to_string(),
            summary: "Save query: M31".to_string(),
            applied_at: "2026-07-08T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&attr).unwrap();
        let back: AgentAttribution = serde_json::from_str(&json).unwrap();
        assert_eq!(back, attr);
    }

    #[test]
    fn state_variant_is_reachable_for_tests() {
        // Sanity that the imported ProposalState is a real enum value (keeps the
        // test import meaningful under both feature configs).
        assert_ne!(ProposalState::Pending, ProposalState::Applied);
    }
}
