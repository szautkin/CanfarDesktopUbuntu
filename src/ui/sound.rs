//! Two short sounds, so the agent's arrival and departure are noticeable
//! without being watched for.
//!
//! The indicator beside the service health says whether an agent is working;
//! this says WHEN it started and stopped, to someone who is reading a paper in
//! another window. That is the whole scope: two cues, a hundredth of a second
//! of attention each, and a switch to turn them off.
//!
//! Deliberately not a general sound system. There is no queue, no mixing and no
//! per-event volume, because a second sound in this application would be one
//! too many.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::path::PathBuf;

/// The two moments worth a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// An agent has started calling tools.
    AgentStarted,
    /// It has gone quiet again.
    AgentFinished,
}

impl Cue {
    fn file_name(self) -> &'static str {
        match self {
            Cue::AgentStarted => "agent-start.wav",
            Cue::AgentFinished => "agent-stop.wav",
        }
    }
}

/// Where the package installs its sounds.
const INSTALLED_DIR: &str = "/usr/share/verbinal/sounds";

/// The file for `cue`, wherever this build keeps its sounds.
///
/// The build tree first, so a checkout hears its own edits, then the installed
/// location. `None` means neither exists, and a missing sound is a silence, not
/// an error: the app has not failed at anything a person asked it to do.
pub fn file_for(cue: Cue) -> Option<PathBuf> {
    let from_tree = crate::source_tree_asset("sounds").map(|d| d.join(cue.file_name()));
    from_tree
        .filter(|p| p.is_file())
        .or_else(|| Some(PathBuf::from(INSTALLED_DIR).join(cue.file_name())))
        .filter(|p| p.is_file())
}

thread_local! {
    /// Sounds currently playing.
    ///
    /// A `MediaFile` stops when it is dropped, so playing one and letting it go
    /// out of scope plays nothing at all. Held until it reports it has ended,
    /// then released — the list is normally empty and never longer than the
    /// number of cues, because a cue that is already playing is restarted
    /// rather than layered.
    static PLAYING: RefCell<Vec<gtk::MediaFile>> = const { RefCell::new(Vec::new()) };
}

/// Play `cue`, if sounds are on and the file is there.
///
/// Silent and harmless in every other case: sounds switched off, the file not
/// installed, or no media backend on the machine — GTK's media support is a
/// separate package on Debian and Ubuntu, and an application that refused to
/// run without it would be trading a working app for a ding.
pub fn play(cue: Cue) {
    if !enabled() {
        return;
    }
    let Some(path) = file_for(cue) else {
        return;
    };
    let media = gtk::MediaFile::for_filename(path);
    media.set_volume(0.6);
    {
        let media = media.clone();
        media.clone().connect_ended_notify(move |m| {
            if m.is_ended() {
                PLAYING.with(|p| p.borrow_mut().retain(|x| x != &media));
            }
        });
    }
    PLAYING.with(|p| p.borrow_mut().push(media.clone()));
    media.play();
}

/// Whether the agent cues are switched on.
///
/// Read per cue rather than cached, so turning them off in Preferences takes
/// effect on the next one instead of on the next launch.
pub fn enabled() -> bool {
    crate::services::settings_service::SettingsService::new()
        .load()
        .agent_sounds
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both cues exist, and they are different sounds.
    ///
    /// A start and a finish that sound the same are one sound played twice,
    /// which tells you something happened and not which thing.
    #[test]
    fn the_two_cues_are_two_different_files() {
        let start = file_for(Cue::AgentStarted).expect("the start sound is missing");
        let stop = file_for(Cue::AgentFinished).expect("the finish sound is missing");
        assert_ne!(start, stop);
        let a = std::fs::read(&start).expect("readable");
        let b = std::fs::read(&stop).expect("readable");
        assert_ne!(a, b, "the two cues are the same audio");
    }

    /// They are short, quiet cues rather than a notification jingle.
    ///
    /// Measured off the file rather than trusted: a cue long enough to be a
    /// tune is a cue people switch off, and the switch is not the point.
    #[test]
    fn a_cue_is_under_half_a_second() {
        for cue in [Cue::AgentStarted, Cue::AgentFinished] {
            let path = file_for(cue).expect("present");
            let bytes = std::fs::read(&path).expect("readable");
            // Canonical WAV: 44 byte header, then 16-bit mono PCM.
            let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
            let frames = (bytes.len() - 44) / 2;
            let seconds = frames as f64 / f64::from(rate);
            assert!(
                seconds < 0.5,
                "{cue:?} runs for {seconds:.2}s, which is a tune, not a cue"
            );
        }
    }

    /// The package installs what the app looks for.
    ///
    /// The lookup falls back to `/usr/share/verbinal/sounds`, and a fallback to
    /// a path nothing ever puts a file in is a silence on every machine but a
    /// developer's.
    #[test]
    fn the_package_ships_both_cues() {
        let cargo = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("Cargo.toml is readable");
        for cue in [Cue::AgentStarted, Cue::AgentFinished] {
            let name = cue.file_name();
            assert!(
                cargo.contains(&format!("assets/sounds/{name}")),
                "the deb does not install {name}"
            );
        }
        assert!(
            cargo.contains(&INSTALLED_DIR[1..]),
            "the deb installs the sounds somewhere other than {INSTALLED_DIR}, \
             which is where `file_for` looks"
        );
        // And the thing that plays them is asked for. Recommended rather than
        // required — see the comment beside it — but an installed app whose
        // sounds are silent on every machine is a feature that does not exist.
        assert!(
            cargo.contains("libgtk-4-media-gstreamer"),
            "nothing in the package asks for a media backend, so the cues would \
             be silent wherever one is not already installed"
        );
    }
}
