//! What the kernel is doing, as one value.
//!
//! The status used to be a single English sentence — `"Kernel: idle"` — doing
//! three jobs at once. It was the text on screen, it was the colour of the dot
//! (found by searching it for the word "idle"), and it was the `state` field
//! `get_kernel_state` reports over MCP (found by searching it again).
//!
//! Three consumers of one string, two of them by substring match, means the
//! string cannot be translated without breaking the other two. So it was not
//! translated — except for the one place that set the FIRST label through
//! `tr_en!`, which is why a French desktop opened a notebook showing
//! "Noyau : non démarré" and then switched to English the moment anything
//! happened. Half-translated was the only stable point that arrangement had.
//!
//! Here the state is the value and the strings are derived from it: a stable
//! keyword that never changes with locale, an English line for the API, and a
//! translated line for the window. Nothing parses anything.

/// The kernel's current state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelStatus {
    /// No kernel process yet. The state a notebook opens in.
    NotStarted,
    Starting,
    Restarting,
    Idle,
    Busy,
    /// Busy, and past the user's configured warning threshold.
    BusySlow {
        seconds: u64,
    },
    /// A cell could not be run; the kernel itself may still be alive.
    Error(String),
    /// The kernel could not be started at all.
    Failed(String),
}

impl KernelStatus {
    /// The stable keyword: dot colour, and the MCP `state` field.
    ///
    /// Never translated and never derived from prose. This is the contract an
    /// agent branches on, and a French desktop must not change it.
    pub fn keyword(&self) -> &'static str {
        match self {
            Self::NotStarted => "dead",
            Self::Starting | Self::Restarting => "starting",
            Self::Idle => "idle",
            Self::Busy | Self::BusySlow { .. } => "busy",
            Self::Error(_) | Self::Failed(_) => "error",
        }
    }

    /// The English line, for the API.
    ///
    /// `get_kernel_state` is read by programs. Handing it whatever language the
    /// desktop happens to be in would make the reply depend on the operator's
    /// locale, so `statusText` stays English and `keyword` stays stable; the
    /// translated line below is for the window.
    pub fn api_text(&self) -> String {
        match self {
            Self::NotStarted => "Kernel: not started".to_string(),
            Self::Starting => "Kernel: starting…".to_string(),
            Self::Restarting => "Kernel: restarting…".to_string(),
            Self::Idle => "Kernel: idle".to_string(),
            Self::Busy => "Kernel: busy".to_string(),
            Self::BusySlow { seconds } => {
                format!("Kernel: busy — cell running over {seconds}s (press I,I to Interrupt)")
            }
            Self::Error(detail) => format!("Kernel: error — {detail}"),
            Self::Failed(detail) => format!("Kernel: failed — {detail}"),
        }
    }

    /// The line a person reads, in their language.
    pub fn label(&self) -> String {
        match self {
            Self::NotStarted => crate::tr_en!("Kernel: not started").to_string(),
            Self::Starting => crate::tr_en!("Kernel: starting…").to_string(),
            Self::Restarting => crate::tr_en!("Kernel: restarting…").to_string(),
            Self::Idle => crate::tr_en!("Kernel: idle").to_string(),
            Self::Busy => crate::tr_en!("Kernel: busy").to_string(),
            Self::BusySlow { seconds } => crate::tr_fmt!(
                "Kernel: busy — cell running over {}s (press I,I to Interrupt)",
                seconds
            ),
            Self::Error(detail) => crate::tr_fmt!("Kernel: error — {}", detail),
            Self::Failed(detail) => crate::tr_fmt!("Kernel: failed — {}", detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state maps to one of the keywords the UI and MCP already use.
    #[test]
    fn keywords_are_the_agreed_set() {
        const KNOWN: [&str; 5] = ["dead", "starting", "idle", "busy", "error"];
        for status in [
            KernelStatus::NotStarted,
            KernelStatus::Starting,
            KernelStatus::Restarting,
            KernelStatus::Idle,
            KernelStatus::Busy,
            KernelStatus::BusySlow { seconds: 30 },
            KernelStatus::Error("boom".into()),
            KernelStatus::Failed("no python".into()),
        ] {
            assert!(
                KNOWN.contains(&status.keyword()),
                "{status:?} produced the unknown keyword {:?}",
                status.keyword()
            );
        }
    }

    /// A busy kernel is busy whether or not it is also slow.
    ///
    /// The old substring match got this right by luck — the slow message
    /// happens to contain the word "busy". Anyone rewording it would have
    /// silently changed the dot and the API state.
    #[test]
    fn a_slow_cell_is_still_busy() {
        assert_eq!(KernelStatus::Busy.keyword(), "busy");
        assert_eq!(KernelStatus::BusySlow { seconds: 30 }.keyword(), "busy");
    }

    /// The API text does not follow the desktop's language.
    ///
    /// `keyword` is what a caller should branch on, but `statusText` is in
    /// every reply, and a reply that changes language with the operator's
    /// locale is a reply a program cannot rely on.
    #[test]
    fn the_api_text_is_english_and_carries_its_detail() {
        assert_eq!(KernelStatus::Idle.api_text(), "Kernel: idle");
        assert_eq!(KernelStatus::NotStarted.api_text(), "Kernel: not started");
        assert!(KernelStatus::Error("division by zero".into())
            .api_text()
            .contains("division by zero"));
        assert!(KernelStatus::BusySlow { seconds: 45 }
            .api_text()
            .contains("45"));
    }

    /// The API text and the keyword do not follow the desktop's language.
    ///
    /// Asserting this in an English test process proves nothing — every string
    /// is English there whether or not it went through the translator. So this
    /// switches the app to French, which is the condition the bug appeared
    /// under: a French desktop answered `create_notebook` with
    /// "Noyau : non démarré" and `get_kernel_state` with "Kernel: idle".
    ///
    /// The window's label SHOULD change; the API must not.
    #[test]
    fn french_changes_the_label_and_leaves_the_api_alone() {
        use crate::i18n::{set_lang, Lang};

        // Serialised against the other locale-switching tests: `CURRENT` is
        // process-wide, so two tests flipping it at once would each see the
        // other's language.
        let _guard = crate::i18n::testing_lang_lock();

        set_lang(Lang::Fr);
        let label = KernelStatus::Idle.label();
        let api = KernelStatus::Idle.api_text();
        let keyword = KernelStatus::Idle.keyword();
        set_lang(Lang::En);

        assert_eq!(label, "Noyau : inactif", "the window should be translated");
        assert_eq!(api, "Kernel: idle", "the API followed the desktop language");
        assert_eq!(keyword, "idle", "the machine state followed the language");
    }

    /// The keyword survives a reworded message, which is the whole point.
    #[test]
    fn the_keyword_does_not_depend_on_the_wording() {
        // An error whose detail happens to contain another state's word.
        let status = KernelStatus::Error("the cell went idle unexpectedly".into());
        assert_eq!(
            status.keyword(),
            "error",
            "the detail text leaked into the state"
        );

        // And a failure whose detail mentions starting.
        let status = KernelStatus::Failed("could not finish starting".into());
        assert_eq!(status.keyword(), "error");
    }
}
