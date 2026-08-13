use serde::{Deserialize, Serialize};

/// Skaha session types the user can launch INTERACTIVELY, in the order the UI
/// offers them.
///
/// These strings go to Skaha verbatim, so they are identifiers, never display
/// labels. They live here rather than in a page because three surfaces need the
/// same list — the launch form builds its dropdown from it, Settings writes one
/// of them into `default_session_type`, and the session strip filters by it.
/// Settings kept its own copy, so a type added to the launcher alone could have
/// been saved as a default the launcher then failed to preselect.
pub const INTERACTIVE_SESSION_TYPES: [&str; 5] =
    ["notebook", "desktop", "carta", "contributed", "firefly"];

/// Everything the Advanced tab can submit: the interactive types plus batch.
///
/// Also the `list_session_types` payload and the image-discovery filter enum.
/// Those carried their own copies, one of them with `firefly` and `contributed`
/// the other way round — two tools advertising the same enum in different
/// orders, which is what a second copy looks like before it becomes a
/// disagreement about contents.
pub const LAUNCHABLE_SESSION_TYPES: [&str; 6] = [
    "notebook",
    "desktop",
    "carta",
    "contributed",
    "firefly",
    "headless",
];

#[derive(Debug, Clone, Deserialize)]
pub struct SkahaSessionResponse {
    pub id: String,
    pub userid: Option<String>,
    pub image: Option<String>,
    #[serde(rename = "type")]
    pub session_type: Option<String>,
    pub status: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(rename = "expiryTime")]
    pub expiry_time: Option<String>,
    #[serde(rename = "connectURL")]
    pub connect_url: Option<String>,
    #[serde(rename = "requestedRAM")]
    pub requested_ram: Option<String>,
    #[serde(rename = "requestedCPUCores")]
    pub requested_cpu_cores: Option<String>,
    #[serde(rename = "requestedGPUCores")]
    pub requested_gpu_cores: Option<String>,
    #[serde(rename = "ramInUse")]
    pub ram_in_use: Option<String>,
    #[serde(rename = "cpuCoresInUse")]
    pub cpu_cores_in_use: Option<String>,
    #[serde(rename = "isFixedResources")]
    pub is_fixed_resources: Option<bool>,
}

/// `PartialEq` is load-bearing, not incidental: the session strip polls every
/// 15s, and comparing the new snapshot against the rendered one is what lets it
/// leave unchanged cards alone instead of rebuilding the whole strip — which
/// reset the user's scroll position and dropped hover four times a minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub userid: String,
    pub image: String,
    pub session_type: String,
    pub status: String,
    pub name: String,
    pub start_time: String,
    pub expiry_time: String,
    pub connect_url: String,
    pub requested_ram: String,
    pub requested_cpu_cores: String,
    pub requested_gpu_cores: String,
    pub ram_in_use: String,
    pub cpu_cores_in_use: String,
    pub is_fixed_resources: bool,
}

impl From<SkahaSessionResponse> for Session {
    fn from(r: SkahaSessionResponse) -> Self {
        Session {
            id: r.id,
            userid: r.userid.unwrap_or_default(),
            image: r.image.unwrap_or_default(),
            session_type: r.session_type.unwrap_or_default(),
            status: r.status.unwrap_or_default(),
            name: r.name.unwrap_or_default(),
            start_time: r.start_time.unwrap_or_default(),
            expiry_time: r.expiry_time.unwrap_or_default(),
            connect_url: r.connect_url.unwrap_or_default(),
            requested_ram: r.requested_ram.unwrap_or_default(),
            requested_cpu_cores: r.requested_cpu_cores.unwrap_or_default(),
            requested_gpu_cores: r.requested_gpu_cores.unwrap_or("0".into()),
            ram_in_use: r.ram_in_use.unwrap_or_default(),
            cpu_cores_in_use: r.cpu_cores_in_use.unwrap_or_default(),
            is_fixed_resources: r.is_fixed_resources.unwrap_or(true),
        }
    }
}

impl Session {
    pub fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("running")
    }

    pub fn is_pending(&self) -> bool {
        self.status.eq_ignore_ascii_case("pending")
    }

    /// A headless (batch) job — has no interactive Open URL and must NOT count
    /// toward the interactive-session cap or appear in the Active Sessions strip.
    pub fn is_headless(&self) -> bool {
        self.session_type.eq_ignore_ascii_case("headless")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_advanced_tab_offers_everything_the_interactive_list_does_plus_batch() {
        // The two lists differ on purpose — only the Advanced tab submits batch
        // jobs — but it must not silently LOSE an interactive type when one is
        // added to the shared list.
        for session_type in INTERACTIVE_SESSION_TYPES {
            assert!(
                LAUNCHABLE_SESSION_TYPES.contains(&session_type),
                "`{session_type}` can be launched interactively but not from Advanced"
            );
        }
        assert!(
            LAUNCHABLE_SESSION_TYPES.contains(&"headless"),
            "the Advanced tab is the only route to a batch job"
        );
        assert!(
            !INTERACTIVE_SESSION_TYPES.contains(&"headless"),
            "a batch job is not an interactive session and has no Open URL"
        );
    }

    #[test]
    fn a_session_type_is_a_skaha_identifier_not_a_display_label() {
        // These strings go to Skaha verbatim. A translated or capitalised label
        // here would be rejected by the platform.
        for session_type in LAUNCHABLE_SESSION_TYPES {
            assert_eq!(
                session_type,
                session_type.to_lowercase(),
                "`{session_type}` is sent to Skaha as-is"
            );
            assert!(
                !session_type.contains(' '),
                "`{session_type}` is sent to Skaha as-is"
            );
        }
    }

    #[test]
    fn is_headless_agrees_with_the_launchable_list() {
        // `is_headless` decides whether a session gets a card and counts toward
        // the interactive cap, so it has to recognise the exact string the
        // launcher submits.
        let mut session = Session::from(SkahaSessionResponse {
            id: "1".to_string(),
            userid: None,
            image: None,
            session_type: Some("headless".to_string()),
            status: None,
            name: None,
            start_time: None,
            expiry_time: None,
            connect_url: None,
            requested_ram: None,
            requested_cpu_cores: None,
            requested_gpu_cores: None,
            ram_in_use: None,
            cpu_cores_in_use: None,
            is_fixed_resources: None,
        });
        assert!(session.is_headless());

        for interactive in INTERACTIVE_SESSION_TYPES {
            session.session_type = interactive.to_string();
            assert!(
                !session.is_headless(),
                "`{interactive}` is interactive and must render as a card"
            );
        }
    }

    /// A session must compare unequal when anything the card SHOWS changes.
    ///
    /// The session strip skips its rebuild when the polled sessions equal the
    /// rendered ones, so a field that failed to participate in equality would
    /// leave a stale card on screen — a session shown as Running long after it
    /// died. Adding `#[serde(skip)]` or a non-comparable field to `Session`
    /// should fail here.
    #[test]
    fn a_changed_session_compares_unequal_so_its_card_redraws() {
        let json = r#"{
            "id": "abc123",
            "userid": "testuser",
            "image": "images.canfar.net/skaha/notebook:1.0",
            "type": "notebook",
            "status": "Running",
            "name": "notebook1",
            "startTime": "2024-01-15T10:00:00Z",
            "expiryTime": "2024-01-22T10:00:00Z",
            "connectURL": "https://example.com/session/abc123",
            "requestedRAM": "8G",
            "requestedCPUCores": "2",
            "requestedGPUCores": "0"
        }"#;
        let parsed: SkahaSessionResponse = serde_json::from_str(json).unwrap();
        let a = Session::from(parsed.clone());

        // Identical payload → equal, so the strip leaves the card alone.
        assert_eq!(a, Session::from(parsed.clone()));

        // Every field the card renders must break equality when it changes.
        for mutate in [
            "\"status\": \"Running\"",
            "\"requestedRAM\": \"8G\"",
            "\"requestedCPUCores\": \"2\"",
            "\"expiryTime\": \"2024-01-22T10:00:00Z\"",
            "\"name\": \"notebook1\"",
        ] {
            let changed = json.replace(
                mutate,
                &mutate
                    .replace("8G", "16G")
                    .replace("Running", "Terminating")
                    .replace("\"2\"", "\"4\"")
                    .replace("2024-01-22", "2024-01-29")
                    .replace("notebook1", "notebook2"),
            );
            let b: SkahaSessionResponse = serde_json::from_str(&changed).unwrap();
            assert_ne!(
                a,
                Session::from(b),
                "changing {mutate} must make the session compare unequal"
            );
        }
    }

    #[test]
    fn deserialize_skaha_response() {
        let json = r#"{
            "id": "abc123",
            "userid": "testuser",
            "image": "images.canfar.net/skaha/notebook:1.0",
            "type": "notebook",
            "status": "Running",
            "name": "notebook1",
            "startTime": "2024-01-15T10:00:00Z",
            "expiryTime": "2024-01-22T10:00:00Z",
            "connectURL": "https://example.com/session/abc123",
            "requestedRAM": "8G",
            "requestedCPUCores": "2",
            "requestedGPUCores": "0",
            "ramInUse": "4G",
            "cpuCoresInUse": "1",
            "isFixedResources": true
        }"#;

        let resp: SkahaSessionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "abc123");
        assert_eq!(resp.session_type.as_deref(), Some("notebook"));
        assert_eq!(
            resp.connect_url.as_deref(),
            Some("https://example.com/session/abc123")
        );
        assert_eq!(resp.requested_ram.as_deref(), Some("8G"));
        assert_eq!(resp.requested_cpu_cores.as_deref(), Some("2"));
        assert_eq!(resp.requested_gpu_cores.as_deref(), Some("0"));
        assert_eq!(resp.is_fixed_resources, Some(true));
    }

    #[test]
    fn deserialize_minimal_response() {
        let json = r#"{"id": "xyz"}"#;
        let resp: SkahaSessionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "xyz");
        assert!(resp.session_type.is_none());
        assert!(resp.connect_url.is_none());
    }

    #[test]
    fn session_from_response_defaults() {
        let resp = SkahaSessionResponse {
            id: "abc".to_string(),
            userid: None,
            image: None,
            session_type: None,
            status: None,
            name: None,
            start_time: None,
            expiry_time: None,
            connect_url: None,
            requested_ram: None,
            requested_cpu_cores: None,
            requested_gpu_cores: None,
            ram_in_use: None,
            cpu_cores_in_use: None,
            is_fixed_resources: None,
        };
        let session = Session::from(resp);
        assert_eq!(session.id, "abc");
        assert_eq!(session.userid, "");
        assert_eq!(session.requested_gpu_cores, "0");
        assert!(session.is_fixed_resources);
    }

    #[test]
    fn session_status_checks() {
        let mut session = Session::from(SkahaSessionResponse {
            id: "1".to_string(),
            userid: None,
            image: None,
            session_type: None,
            status: Some("Running".to_string()),
            name: None,
            start_time: None,
            expiry_time: None,
            connect_url: None,
            requested_ram: None,
            requested_cpu_cores: None,
            requested_gpu_cores: None,
            ram_in_use: None,
            cpu_cores_in_use: None,
            is_fixed_resources: None,
        });
        assert!(session.is_running());
        assert!(!session.is_pending());

        session.status = "Pending".to_string();
        assert!(!session.is_running());
        assert!(session.is_pending());

        session.status = "RUNNING".to_string();
        assert!(session.is_running());
    }
}

#[cfg(test)]
mod session_type_tests {
    use super::{INTERACTIVE_SESSION_TYPES, LAUNCHABLE_SESSION_TYPES};

    #[test]
    fn the_launchable_set_is_the_interactive_set_plus_headless() {
        // The two lists are related by a rule, so state it: adding an
        // interactive type and forgetting the other list is how they drifted
        // into five copies in the first place.
        for kind in INTERACTIVE_SESSION_TYPES {
            assert!(
                LAUNCHABLE_SESSION_TYPES.contains(&kind),
                "`{kind}` is interactive but not launchable"
            );
        }
        assert!(LAUNCHABLE_SESSION_TYPES.contains(&"headless"));
        assert_eq!(
            LAUNCHABLE_SESSION_TYPES.len(),
            INTERACTIVE_SESSION_TYPES.len() + 1
        );
    }

    /// Every file that has held a copy of this list, or plausibly could.
    const SESSION_TYPE_READERS: &[(&str, &str)] = &[
        ("ui/launch_form.rs", include_str!("../ui/launch_form.rs")),
        (
            "ui/settings_page.rs",
            include_str!("../ui/settings_page.rs"),
        ),
        (
            "helpers/image_parser.rs",
            include_str!("../helpers/image_parser.rs"),
        ),
        (
            "mcp/tools/sessions.rs",
            include_str!("../mcp/tools/sessions.rs"),
        ),
        ("mcp/tools/write.rs", include_str!("../mcp/tools/write.rs")),
        (
            "mcp/tools/imagediscovery.rs",
            include_str!("../mcp/tools/imagediscovery.rs"),
        ),
    ];

    #[test]
    fn nobody_else_writes_the_list_out() {
        // It reached FIVE copies, one of them in a different order, and the
        // launch form decoded a dropdown against three private ones — the shape
        // that made a search in Ångström run in centimetres, in a different
        // room. A source scan, because a second copy compiles perfectly and is
        // wrong only later.
        //
        let needle = format!("{:?},\n", INTERACTIVE_SESSION_TYPES[0]);
        for (name, source) in SESSION_TYPE_READERS {
            // Tests stripped, so a guard can never be its own evidence — the
            // trap that has caught five of these. See `crate::testing::code`.
            let body = crate::testing::code(source);
            for (at, _) in body.match_indices(&needle) {
                let after = &body[at..(at + 200).min(body.len())];
                assert!(
                    !after.contains(&format!("{:?}", INTERACTIVE_SESSION_TYPES[1])),
                    "{name} writes the session-type list out again; decode it \
                     from models::session instead"
                );
            }
        }
    }

    #[test]
    fn headless_is_not_an_interactive_type() {
        // The MCP surface tells the two apart, and a batch job in the
        // interactive list would be launched down a path that expects a URL.
        assert!(!INTERACTIVE_SESSION_TYPES.contains(&"headless"));
    }
}
