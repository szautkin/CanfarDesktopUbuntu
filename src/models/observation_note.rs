//! A user's research annotation for a saved observation, keyed by publisher ID.
//!
//! Ported from `Models/ObservationNote.cs`.  In the Windows reference these
//! live in SQLite with an FTS5 index; the Linux port stores them in a small
//! JSON map (`observation_notes.json`) and does a plain substring search as
//! the full-text-search substitute (see `services::observation_note_store`).

use crate::helpers::agent_attribution::AgentAttribution;
use serde::{Deserialize, Serialize};

/// A rating / free-text note / tag set the user has attached to a single
/// observation.  Identified by the CADC publisher DID
/// (e.g. `ivo://cadc.nrc.ca/CFHT?123456`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObservationNote {
    /// CADC publisher DID this note belongs to.
    pub publisher_id: String,
    /// Star rating, 0–5 (0 = unrated).
    #[serde(default)]
    pub rating: u8,
    /// Free-text note body.
    #[serde(default)]
    pub note: String,
    /// Free-form tags (stored trimmed, non-empty).
    #[serde(default)]
    pub tags: Vec<String>,
    /// ISO-8601 timestamp of the last edit (UTC).
    #[serde(default)]
    pub updated: String,
    /// Provenance stamp when this note was written via an applied agent proposal.
    /// `None` for user-authored notes; a `Some(..)` value drives the wand badge.
    /// `#[serde(default)]` keeps pre-attribution JSON readable.
    #[serde(default)]
    pub agent_attribution: Option<AgentAttribution>,
}

impl ObservationNote {
    /// True when there is nothing worth persisting: blank note, unrated, no
    /// tags.  Saving an empty note removes the row (mirrors the Windows
    /// `ObservationNote.IsEmpty` + `Upsert` delete-on-empty behavior).
    pub fn is_empty(&self) -> bool {
        self.note.trim().is_empty() && self.rating == 0 && self.tags.is_empty()
    }
}
