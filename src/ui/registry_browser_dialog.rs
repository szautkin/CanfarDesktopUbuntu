//! Search the container registry, and keep a list of what you found.
//!
//! The images widget shows what the platform publishes. That is a curated list,
//! and until now it was the only list: an image the platform had not picked
//! up — a colleague's build, a tag newer than Skaha's catalogue — could not be
//! reached from the app at all, however well the user knew its name.
//!
//! This is the other door, and it opens only when pushed. Nothing here runs on
//! a timer or at start-up: a search is one deliberate act, and what it finds is
//! kept only if the user says so. Enumerating a Harbor instance to fill a
//! dashboard card would be a great deal of traffic to answer a question nobody
//! asked, so the module that talks to the registry refuses an empty search
//! outright.
//!
//! One dialog, two sections, one row builder:
//!
//!   * **Search** — a host, optional credentials, a term. Results are rows with
//!     an **Add** button.
//!   * **Your images** — what has been added, the same rows with **Remove**.
//!
//! The rows are the same rows on purpose. An image in both places is one image,
//! and giving each section its own row would be two places to keep the Add /
//! Remove state honest — which is exactly the sort of pair that drifts.

use crate::helpers::tasks::TaskKind;
use crate::models::RegistryImage;
use crate::services::image_discovery_settings_service::ImageDiscoverySettingsService;
use crate::services::registry_service::RegistryAuth;
use crate::state::AppServices;
use crate::ui::busy::Working;
use crate::ui::dialog::Dialog;
use crate::ui::{fit, space};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// How tall the dialog may get before its content scrolls.
const DIALOG_HEIGHT: i32 = 720;

/// Width reserved for the Add / Remove button.
///
/// The two labels are different lengths, and the button swaps between them in
/// place when an image is added. Without a fixed width the row's contents shift
/// sideways under the pointer that just clicked it.
const ACTION_BTN_WIDTH: i32 = 96;

/// How many results to show.
///
/// The registry service already bounds what it fetches; this bounds what is
/// built into widgets. GTK4's `ListBox` does not virtualise, and the images
/// widget has already measured what a few hundred live rows cost per layout
/// pass.
const MAX_RESULTS: usize = 60;

/// Everything the dialog needs to redraw itself.
struct BrowserUi {
    services: Arc<AppServices>,
    host_entry: adw::EntryRow,
    user_entry: adw::EntryRow,
    secret_entry: adw::PasswordEntryRow,
    search_entry: gtk::SearchEntry,
    status: gtk::Label,
    results_box: gtk::ListBox,
    results_group: adw::PreferencesGroup,
    mine_box: gtk::ListBox,
    mine_group: adw::PreferencesGroup,
    /// The last search's results, kept so a row can be redrawn after an add or
    /// a remove without asking the registry again.
    results: RefCell<Vec<RegistryImage>>,
    /// Called after the user's list changes, so the images widget behind the
    /// dialog catches up.
    on_changed: Rc<dyn Fn()>,
}

/// Open the browser over `parent`. `on_changed` runs whenever the user's list
/// changes, so the caller can refresh whatever it shows.
pub fn show_registry_browser_dialog(
    parent: &impl IsA<gtk::Widget>,
    services: Arc<AppServices>,
    on_changed: Rc<dyn Fn()>,
) {
    let dialog = Dialog::new(
        crate::tr_en!("Add image from registry"),
        fit::BROWSE,
        DIALOG_HEIGHT,
    );

    // Settings the app already holds: someone who configured image discovery
    // has configured this too, and should not be asked twice.
    let settings = ImageDiscoverySettingsService::new();
    let configured = settings.settings().clone();

    // ── Where to look, and as whom ────────────────────────────────────────
    let where_group = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Registry"))
        .build();

    let host_entry = adw::EntryRow::builder()
        .title(crate::tr_en!("Host"))
        .build();
    host_entry.set_text(&configured.registry_host);
    where_group.add(&host_entry);

    // Collapsed by default. A public project needs no credentials, and the
    // common case — a user whose Harbor secret is already in the keychain —
    // needs nothing typed here at all.
    let creds = adw::ExpanderRow::builder()
        .title(crate::tr_en!("Credentials"))
        .subtitle(if settings.has_secret() {
            crate::tr_en!("Using your saved Harbor CLI secret")
        } else {
            crate::tr_en!("Optional — public projects need none")
        })
        .build();

    let user_entry = adw::EntryRow::builder()
        .title(crate::tr_en!("Username"))
        .build();
    user_entry.set_text(&configured.username);
    creds.add_row(&user_entry);

    let secret_entry = adw::PasswordEntryRow::builder()
        .title(crate::tr_en!("Harbor CLI secret"))
        .build();
    creds.add_row(&secret_entry);
    where_group.add(&creds);
    dialog.content().append(&where_group);

    // ── The search ────────────────────────────────────────────────────────
    let search_group = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Search the registry"))
        .description(crate::tr_en!(
            "Images are fetched only when you search. Nothing is downloaded in the background."
        ))
        .build();

    let search_row = gtk::Box::new(gtk::Orientation::Horizontal, space::CONTROL);
    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some(crate::tr_en!("Repository name, e.g. astroml")));
    search_entry.set_hexpand(true);
    let search_btn = gtk::Button::with_label(crate::tr_en!("Search"));
    search_btn.add_css_class("suggested-action");
    search_btn.set_valign(gtk::Align::Center);
    search_row.append(&search_entry);
    search_row.append(&search_btn);
    search_group.add(&search_row);
    dialog.content().append(&search_group);

    let status = gtk::Label::new(None);
    status.add_css_class("dim-label");
    status.set_halign(gtk::Align::Start);
    status.set_wrap(true);
    dialog.content().append(&status);

    let results_group = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Results"))
        .build();
    let results_box = list_box();
    results_group.add(&results_box);
    results_group.set_visible(false);
    dialog.content().append(&results_group);

    // ── What the user has already added ───────────────────────────────────
    let mine_group = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Your images"))
        .description(crate::tr_en!(
            "Added images appear in the CANFAR Images list, in Find images by package, and in the launch form."
        ))
        .build();
    let mine_box = list_box();
    mine_group.add(&mine_box);
    dialog.content().append(&mine_group);

    let ui = Rc::new(BrowserUi {
        services,
        host_entry,
        user_entry,
        secret_entry,
        search_entry: search_entry.clone(),
        status,
        results_box,
        results_group,
        mine_box,
        mine_group,
        results: RefCell::new(Vec::new()),
        on_changed,
    });
    ui.rebuild_mine();

    // Search on the button, and on Enter in the entry — the same act, so the
    // same code path.
    {
        let ui = ui.clone();
        let btn = search_btn.clone();
        search_btn.connect_clicked(move |_| ui.clone().search(&btn));
    }
    {
        let ui = ui.clone();
        let btn = search_btn.clone();
        search_entry.connect_activate(move |_| ui.clone().search(&btn));
    }

    let close = gtk::Button::with_label(crate::tr_en!("Close"));
    {
        let window = dialog.window.clone();
        close.connect_clicked(move |_| window.close());
    }
    dialog.add_action(&close);
    dialog.present(parent);
    ui.search_entry.grab_focus();
}

/// A boxed list, as every other list in this app is drawn.
fn list_box() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list
}

impl BrowserUi {
    /// Ask the registry, then redraw the results.
    fn search(self: Rc<Self>, button: &gtk::Button) {
        let term = self.search_entry.text().to_string();
        let host = self.host_entry.text().to_string();
        let auth = self.auth();

        // The registered task is what puts this in the status bar: a registry
        // that is slow to answer should be visible as work in progress, not as
        // a dialog that has stopped responding.
        let working = Working::start(button, TaskKind::Discovery, {
            let term = term.trim();
            if term.is_empty() {
                crate::tr_en!("Registry search").to_string()
            } else {
                crate::tr_fmt!("Registry search: {}", term)
            }
        });

        let ui = self.clone();
        glib::spawn_future_local(async move {
            ui.status.set_text(crate::tr_en!("Searching the registry…"));

            let svc = ui.services.clone();
            let result = ui
                .services
                .spawn(async move { svc.registry.search(&host, &term, &auth).await })
                .await;
            working.finish(&result);

            match result {
                Ok(found) if found.is_empty() => {
                    ui.status
                        .set_text(crate::tr_en!("Nothing in the registry matched that."));
                    ui.results_group.set_visible(false);
                    ui.results.borrow_mut().clear();
                }
                Ok(found) => {
                    ui.status.set_text(&crate::tr_plural!(
                        found.len(),
                        "{} image found",
                        "{} images found"
                    ));
                    *ui.results.borrow_mut() = found;
                    ui.rebuild_results();
                }
                Err(e) => {
                    ui.status.set_text(&e);
                    ui.results_group.set_visible(false);
                }
            }
        });
    }

    /// Credentials for the search: what was typed, else what is stored.
    ///
    /// Typed ones win so a user can try a different account without disturbing
    /// the secret their image discovery runs on. They are never written
    /// anywhere — this dialog holds them for as long as it is open, no longer.
    fn auth(&self) -> RegistryAuth {
        let typed = RegistryAuth::from_credentials(
            &self.user_entry.text(),
            self.secret_entry.text().as_str(),
        );
        if typed.basic.is_some() {
            return typed;
        }
        RegistryAuth::from_basic(ImageDiscoverySettingsService::new().current_auth_header())
    }

    fn rebuild_results(self: &Rc<Self>) {
        clear(&self.results_box);
        // One snapshot for the whole rebuild: asking the store per row took its
        // lock sixty times to answer sixty one-word questions.
        let added = self.added_ids();
        let results = self.results.borrow();
        for image in results.iter().take(MAX_RESULTS) {
            self.results_box
                .append(&self.build_row(image, added.contains(&image.id)));
        }
        self.results_group.set_visible(!results.is_empty());
    }

    fn rebuild_mine(self: &Rc<Self>) {
        clear(&self.mine_box);
        let mine = self.services.user_images.list();
        for image in &mine {
            // Every row here is by definition in the list.
            self.mine_box.append(&self.build_row(image, true));
        }
        self.mine_group.set_visible(!mine.is_empty());
        self.mine_group.set_title(&if mine.is_empty() {
            crate::tr_en!("Your images").to_string()
        } else {
            crate::tr_fmt!("Your images ({})", mine.len())
        });
    }

    /// The ids in the user's list, for one rebuild.
    fn added_ids(&self) -> std::collections::HashSet<String> {
        self.services
            .user_images
            .list()
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    /// One image, in either section.
    ///
    /// The same builder for both, so an image cannot show Add in one place and
    /// Remove in the other: `added` decides the button, and both callers derive
    /// it from the same store rather than from which list the row is in.
    fn build_row(self: &Rc<Self>, image: &RegistryImage, added: bool) -> adw::ActionRow {
        let row = adw::ActionRow::new();
        row.set_use_markup(false);
        row.set_title(&image.id);
        row.set_subtitle(&type_summary(image));
        row.set_subtitle_lines(0);

        let button = gtk::Button::with_label(if added {
            crate::tr_en!("Remove")
        } else {
            crate::tr_en!("Add")
        });
        button.set_valign(gtk::Align::Center);
        button.set_size_request(ACTION_BTN_WIDTH, -1);
        if added {
            button.add_css_class("destructive-action");
        } else {
            button.add_css_class("suggested-action");
        }

        {
            let ui = self.clone();
            let image = image.clone();
            button.connect_clicked(move |_| ui.clone().toggle(&image, added));
        }
        row.add_suffix(&button);
        row
    }

    /// Add the image, or take it out again.
    fn toggle(self: Rc<Self>, image: &RegistryImage, was_added: bool) {
        let store = &self.services.user_images;
        let outcome = if was_added {
            store.remove(&image.id)
        } else {
            store.add(image.clone())
        };

        match outcome {
            Ok(()) => {
                // Both lists: the same image can be in both, and the button in
                // the other one is now wrong.
                self.rebuild_mine();
                self.rebuild_results();
                (self.on_changed)();
                self.services.toast.toast(if was_added {
                    crate::tr_fmt!("Removed {}", image.id)
                } else {
                    crate::tr_fmt!("Added {}", image.id)
                });
            }
            Err(e) => {
                // Said in the dialog rather than only as a toast: the row the
                // user clicked has not changed, and they need to know why.
                self.status
                    .set_text(&crate::tr_fmt!("Could not save your image list: {}", e));
            }
        }
    }
}

/// What a row says under the image name.
fn type_summary(image: &RegistryImage) -> String {
    if image.types.is_empty() {
        // Not a failure, and worth saying plainly: it is still launchable from
        // the Advanced tab, which takes an image reference directly.
        return crate::tr_en!("No session-type labels — launch from the Advanced tab").to_string();
    }
    let types: Vec<String> = image
        .types
        .iter()
        .map(|t| crate::models::session::type_label(t).to_string())
        .collect();
    types.join(", ")
}

fn clear(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlabelled_image_is_described_rather_than_left_blank() {
        // A blank subtitle reads as a broken row. This one is fine — it just
        // cannot be typed, and the user needs to know where it IS launchable.
        let img = RegistryImage::new("h/p/n:1", &[]);
        let text = type_summary(&img);
        assert!(!text.is_empty());
        assert!(text.contains("Advanced"), "{text}");
    }

    #[test]
    fn a_labelled_image_lists_its_types_the_way_the_rest_of_the_app_names_them() {
        // `type_label` is what the filter bar and the session cards use. A row
        // saying "desktop-app" while the widget says "Desktop" would be two
        // names for one thing.
        let img = RegistryImage::new("h/p/n:1", &["notebook".into(), "carta".into()]);
        let text = type_summary(&img);
        assert!(text.contains(&crate::models::session::type_label("notebook")));
        assert!(text.contains(&crate::models::session::type_label("carta")));
    }

    #[test]
    fn the_registry_is_only_asked_when_the_user_asks() {
        // The rule this whole module exists to keep. A search wired to a timer,
        // to the dialog opening, or to every keystroke would be the background
        // enumeration that was deliberately not built.
        // `code` cuts the file at its first `#[cfg(test)]`, which is what
        // keeps these assertions from matching their own needles.
        let code = crate::testing::without_comments(crate::testing::code(include_str!(
            "registry_browser_dialog.rs"
        )));
        assert!(
            !code.contains("timeout_add") && !code.contains("timeout_future"),
            "something in the registry browser is on a timer"
        );
        assert!(
            !code.contains("connect_search_changed"),
            "the registry is searched on every keystroke"
        );
        // Deliberate acts only: the button, and Enter in the entry.
        assert!(code.contains("connect_clicked"));
        assert!(code.contains("connect_activate"));
    }

    #[test]
    fn typed_credentials_are_never_written_anywhere() {
        // They are for one search, against one account, and the user did not
        // ask to change what their image discovery runs as.
        // `code` cuts the file at its first `#[cfg(test)]`, which is what
        // keeps these assertions from matching their own needles.
        let code = crate::testing::without_comments(crate::testing::code(include_str!(
            "registry_browser_dialog.rs"
        )));
        assert!(
            !code.contains("set_secret") && !code.contains("set_username"),
            "the registry browser writes the credentials typed into it"
        );
    }
}
