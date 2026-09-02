//! Desktop integration: the one name that has to be spelled the same in four
//! places, the addresses the app hands a person, and the icons that have to
//! exist where the package says they do.
//!
//! There is no runtime code here. There is a name — `net.canfar.Verbinal` —
//! which is simultaneously:
//!
//! * the GTK application id, which becomes the Wayland `app_id` of every
//!   window;
//! * the base name of `data/net.canfar.Verbinal.desktop`, which is how the
//!   shell finds the entry for that `app_id`;
//! * the `Icon=` line inside it, which is the icon-theme name to look up;
//! * the file name of every icon under `assets/icons/hicolor/*/apps/`.
//!
//! Break any one of the four and the app launches perfectly with a generic
//! icon. No error, no warning, no log line — which is why this is a test and
//! not a comment.

/// The application id, and therefore the desktop entry, the icon theme name,
/// and the icon file name.
pub const APP_ID: &str = "net.canfar.Verbinal";

/// The project's own page: what the app is, where the builds are, the guides.
///
/// Not `canfar.net` — that is the SERVICE this app talks to, and the two
/// answer different questions. A person opening Help wants this app's page; a
/// person wanting to know what CANFAR is follows the link from there.
pub const PROJECT_URL: &str = "https://verbinal.com";

/// Where the source lives, and therefore where a build comes from.
pub const REPOSITORY_URL: &str = "https://github.com/szautkin/CanfarDesktopUbuntu";

/// Where a bug goes. Reachable from the About window, so the report arrives
/// with a version beside it instead of "the latest one".
pub const ISSUES_URL: &str = "https://github.com/szautkin/CanfarDesktopUbuntu/issues";

/// Where a question goes when it is not a bug.
pub const SUPPORT_URL: &str = "mailto:support@verbinal.com";

#[cfg(test)]
mod tests {
    use super::{APP_ID, ISSUES_URL, PROJECT_URL, REPOSITORY_URL, SUPPORT_URL};
    use std::path::{Path, PathBuf};

    fn repo(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
    }

    fn read(rel: &str) -> String {
        std::fs::read_to_string(repo(rel))
            .unwrap_or_else(|e| panic!("{rel} should be readable: {e}"))
    }

    /// The id the app registers is the id everything else is named after.
    ///
    /// `lib.rs` is the only place the string is chosen; everywhere else copies
    /// it. If someone renames the app, this is the test that says what else has
    /// to move.
    #[test]
    fn the_application_id_is_the_one_everything_is_named_after() {
        let lib = read("src/lib.rs");
        assert!(
            lib.contains(&format!("application_id(\"{APP_ID}\")")),
            "src/lib.rs does not register {APP_ID}"
        );
        assert!(
            repo(&format!("data/{APP_ID}.desktop")).exists(),
            "there is no data/{APP_ID}.desktop for the shell to find"
        );
    }

    /// The desktop entry points at an icon by the same name.
    ///
    /// On Wayland the shell matches a window's `app_id` to this file and reads
    /// `Icon=` from it. A mismatch here is the difference between the app's own
    /// icon and a generic one, with nothing said about it anywhere.
    #[test]
    fn the_desktop_entry_asks_for_the_icon_by_that_name() {
        let desktop = read(&format!("data/{APP_ID}.desktop"));
        let icon = desktop
            .lines()
            .find_map(|l| l.strip_prefix("Icon="))
            .map(str::trim)
            .expect("the desktop entry has no Icon= line at all");
        assert_eq!(
            icon, APP_ID,
            "the desktop entry asks for icon `{icon}`, which is not the app id"
        );
        // Exec has to be on PATH under the packaged name, or the entry launches
        // nothing when a person clicks it.
        assert!(
            desktop.lines().any(|l| l.trim() == "Exec=verbinal"),
            "the desktop entry does not launch `verbinal`"
        );
    }

    /// The AppStream component is the same app, at the version being shipped.
    ///
    /// The metainfo is not compiled and not executed, so nothing else notices
    /// when it drifts: a stale `<release>` means a software centre offers the
    /// previous version's notes for this one, and a mismatched `<id>` means the
    /// listing never joins up with the desktop entry at all.
    #[test]
    fn the_metainfo_describes_this_app_at_this_version() {
        let meta = read(&format!("data/{APP_ID}.metainfo.xml"));
        assert!(
            meta.contains(&format!("<id>{APP_ID}</id>")),
            "the metainfo component id is not {APP_ID}"
        );
        assert!(
            meta.contains(&format!(
                "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
            )),
            "the metainfo does not point at {APP_ID}.desktop, so a store listing \
             cannot launch it"
        );

        let version = read("Cargo.toml")
            .lines()
            .find_map(|l| l.strip_prefix("version = \""))
            .and_then(|v| v.split('"').next())
            .expect("Cargo.toml states a version")
            .to_string();
        assert!(
            meta.contains(&format!("<release version=\"{version}\"")),
            "the metainfo has no <release> for {version}; a software centre would \
             show the previous release's notes for this one"
        );
    }

    /// The addresses in the packaging are the addresses in the code.
    ///
    /// Four files name the project's page and its bug tracker: this module,
    /// `Cargo.toml`, the metainfo, and the README. Nothing compiles the last
    /// three, so a moved page is found by a person clicking About and landing
    /// on a 404 — which is a support ticket, not a build failure.
    #[test]
    fn every_file_that_names_the_project_names_the_same_one() {
        let cargo = read("Cargo.toml");
        assert!(
            cargo.contains(&format!("homepage = \"{PROJECT_URL}\"")),
            "Cargo.toml's homepage is not {PROJECT_URL}"
        );
        assert!(
            cargo.contains(&format!("repository = \"{REPOSITORY_URL}\"")),
            "Cargo.toml's repository is not {REPOSITORY_URL}"
        );

        let meta = read(&format!("data/{APP_ID}.metainfo.xml"));
        // No `contact`: AppStream requires a web page there and this project's
        // contact is a mailbox, so `appstreamcli validate` — a CI gate — refuses
        // it. The About window offers the address instead.
        for (kind, url) in [
            ("homepage", PROJECT_URL),
            ("bugtracker", ISSUES_URL),
            ("vcs-browser", REPOSITORY_URL),
        ] {
            assert!(
                meta.contains(&format!("<url type=\"{kind}\">{url}</url>")),
                "the metainfo's {kind} url is not {url}"
            );
        }

        assert!(
            read("README.md").contains(PROJECT_URL),
            "the README does not link the project page"
        );
    }

    /// The About window offers the page, the tracker and the mailbox.
    ///
    /// `AboutWindow` shows each of these as a link only when it is set; an
    /// unset one is not an error, it is a row that silently is not there.
    #[test]
    fn the_about_window_hands_over_all_three_addresses() {
        let src = read("src/ui/main_window.rs");
        for (setter, url) in [
            (".website(", PROJECT_URL),
            (".issue_url(", ISSUES_URL),
            (".support_url(", SUPPORT_URL),
        ] {
            assert!(
                src.contains(&format!("{setter}crate::desktop_entry::")),
                "the About window does not set {setter}…), so {url} is not offered"
            );
        }
    }

    /// Every image the README shows is in the tree.
    ///
    /// GitHub renders a missing `src` as a broken-image icon and says nothing;
    /// the first person to notice is a stranger deciding whether to install
    /// this. Nothing else reads the README, so nothing else would catch it.
    #[test]
    fn every_image_the_readme_shows_exists() {
        let readme = read("README.md");
        let mut shown = 0usize;
        let mut missing: Vec<String> = Vec::new();
        for piece in readme.split("src=\"").skip(1) {
            let Some(path) = piece.split('"').next() else {
                continue;
            };
            // Only our own files; a badge is somebody else's server.
            if path.starts_with("http") {
                continue;
            }
            shown += 1;
            if !repo(path).exists() {
                missing.push(path.to_string());
            }
        }
        assert!(shown >= 5, "only {shown} screenshots in the README");
        assert!(
            missing.is_empty(),
            "shown but not in the tree: {missing:#?}"
        );
    }

    /// The package ships the metainfo, or it may as well not exist.
    #[test]
    fn the_package_ships_the_metainfo() {
        let cargo = read("Cargo.toml");
        assert!(
            cargo.contains(&format!("data/{APP_ID}.metainfo.xml")),
            "the deb does not install the metainfo, so no software centre sees it"
        );
    }

    /// One main category, or the app appears in the menu more than once.
    ///
    /// `desktop-file-validate` warns about this and CI runs it; the reason it
    /// is here too is that the warning is a HINT, which is easy to walk past.
    #[test]
    fn the_desktop_entry_claims_one_main_category() {
        // The registered main categories, from the desktop-entry spec.
        const MAIN: &[&str] = &[
            "AudioVideo",
            "Audio",
            "Video",
            "Development",
            "Education",
            "Game",
            "Graphics",
            "Network",
            "Office",
            "Science",
            "Settings",
            "System",
            "Utility",
        ];
        let desktop = read(&format!("data/{APP_ID}.desktop"));
        let cats = desktop
            .lines()
            .find_map(|l| l.strip_prefix("Categories="))
            .expect("the desktop entry has categories");
        let claimed: Vec<&str> = cats
            .split(';')
            .filter(|c| !c.is_empty())
            .filter(|c| MAIN.contains(c))
            .collect();
        assert_eq!(
            claimed.len(),
            1,
            "{} main categories ({claimed:?}) — the app would be listed once per category",
            claimed.len()
        );
    }

    /// Every icon the package promises is actually in the tree.
    ///
    /// `cargo deb` copies files by path. A size listed but missing does not
    /// fail the build — it fails at whatever size the shell happens to ask for,
    /// on someone else's panel.
    #[test]
    fn every_icon_the_package_ships_exists() {
        let cargo = read("Cargo.toml");
        let mut shipped = 0usize;
        let mut missing: Vec<String> = Vec::new();
        for line in cargo.lines() {
            let line = line.trim();
            if !line.starts_with("[\"assets/icons/") {
                continue;
            }
            let Some(src) = line.split('"').nth(1) else {
                continue;
            };
            shipped += 1;
            if !repo(src).exists() {
                missing.push(src.to_string());
            }
        }
        assert!(
            shipped >= 8,
            "only {shipped} icons are packaged; the hicolor sizes GNOME asks for are 16 to 512"
        );
        assert!(
            missing.is_empty(),
            "packaged but not in the tree: {missing:#?}"
        );
    }

    /// Every icon in the tree carries the app's name.
    ///
    /// A file named anything else is invisible to the theme lookup however
    /// correct its contents are, so a stray `verbinal.png` beside
    /// `net.canfar.Verbinal.png` is a file nobody will ever see.
    #[test]
    fn every_app_icon_is_named_after_the_app() {
        let root = repo("assets/icons/hicolor");
        let mut wrong: Vec<String> = Vec::new();
        let mut found = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                // Only the `apps` directories hold application icons; the
                // symbolic ones there are our own UI icons, not the app's.
                if !dir.ends_with("apps") {
                    continue;
                }
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if name == APP_ID {
                    found += 1;
                } else if !name.ends_with("-symbolic") {
                    wrong.push(path.display().to_string());
                }
            }
        }
        assert!(found >= 8, "only {found} icons are named {APP_ID}");
        assert!(
            wrong.is_empty(),
            "app icons the theme will never look up, because they are not named \
             {APP_ID}: {wrong:#?}"
        );
    }
}
