//! One finished batch job, kept after CANFAR has forgotten it.
//!
//! Skaha reaps headless jobs, and the image-discovery coordinator deletes its
//! own probe jobs as soon as they finish — success or failure. Between the two,
//! the Portal's Batch Jobs card could show a job fail and then have nothing to
//! say about it a minute later: the job was gone from the listing, its logs and
//! events were gone with it, and the only trace was a count that had ticked
//! from Running to Failed.

use serde::{Deserialize, Serialize};

/// How a job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobOutcome {
    Succeeded,
    Failed,
}

impl JobOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Succeeded => crate::tr_en!("Succeeded"),
            Self::Failed => crate::tr_en!("Failed"),
        }
    }

    /// The libadwaita style class that colours a badge for this outcome.
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Succeeded => "success",
            Self::Failed => "error",
        }
    }
}

/// What launched a job, so the history can tell a probe the app ran on the
/// user's behalf from a job the user submitted themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobOrigin {
    /// A headless job the user launched.
    User,
    /// An image-inspection probe run by the discovery coordinator.
    ImageProbe,
}

impl JobOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => crate::tr_en!("Batch job"),
            Self::ImageProbe => crate::tr_en!("Image inspection"),
        }
    }
}

/// A finished job, as the history remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    /// Skaha session id. Also the de-duplication key.
    pub id: String,
    pub name: String,
    pub image: String,
    pub origin: JobOrigin,
    pub outcome: JobOutcome,
    /// The status string Skaha last reported, kept verbatim — "Failed",
    /// "Succeeded", "Terminating", whatever it actually said.
    pub status: String,
    /// When the job started, if Skaha told us. RFC-3339.
    #[serde(default)]
    pub started_at: String,
    /// When we recorded it as finished. RFC-3339.
    pub finished_at: String,
    /// Why it failed, in as much detail as we could recover: the coordinator's
    /// own diagnosis, plus the tail of the job's logs and events.
    ///
    /// This is the whole point of the history. A status of "Failed" is not a
    /// reason, and by the time anyone reads it the job — and its logs — are
    /// gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// The image being inspected, for [`JobOrigin::ImageProbe`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_image: Option<String>,
}

impl JobRecord {
    /// A one-line summary for a collapsed row.
    pub fn summary(&self) -> String {
        match &self.target_image {
            Some(target) => crate::tr_fmt!("{} — {}", self.origin.label(), target),
            None => self.origin.label().to_string(),
        }
    }
}
