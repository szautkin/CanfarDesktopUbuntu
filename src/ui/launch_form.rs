use crate::helpers::ImageParser;
use crate::models::session::{INTERACTIVE_SESSION_TYPES, LAUNCHABLE_SESSION_TYPES};
use crate::models::{ParsedImage, RecentLaunch, Session, SessionLaunchParams};
use crate::state::AppServices;
use crate::ui::resource_selector::ResourceSelector;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Stack page names for the modal body.
const BODY_FORM: &str = "form";
const BODY_RESULT: &str = "result";

/// Crossfade between the form and its result. Inside the range a modal
/// transition should sit in, and short enough not to delay the close.
const RESULT_FADE_MS: u32 = 200;

/// How big the confirmation glyph is.
const RESULT_ICON_PX: i32 = 48;

/// How long the confirmation stays up before the modal closes itself.
///
/// Long enough to read four words and register that the thing worked, short
/// enough that nobody reaches for the close button first. A launch is a rare,
/// deliberate act — the one moment in this app with any budget for a beat.
const RESULT_DWELL_MS: u32 = 1400;

/// Which of the launch form's three tabs is meant.
///
/// The notebook page index used to be written as a bare `0` / `1` / `2` at
/// every site that switched or interrogated a tab — including the branch that
/// decides WHICH LAUNCH TO PERFORM. Three spellings of the same mapping, none
/// of which said what the number meant. The floating launch button now opens
/// the form on a caller-chosen tab, which would have made that a fourth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTab {
    Standard,
    Advanced,
    Headless,
}

impl LaunchTab {
    /// Tabs in the order the notebook appends them; the index IS the position.
    pub const ORDER: [LaunchTab; 3] = [
        LaunchTab::Standard,
        LaunchTab::Advanced,
        LaunchTab::Headless,
    ];

    /// The notebook page this tab lives on.
    fn page(self) -> u32 {
        match self {
            LaunchTab::Standard => 0,
            LaunchTab::Advanced => 1,
            LaunchTab::Headless => 2,
        }
    }

    /// The tab showing on `page`, or `Standard` for anything unexpected — a
    /// launch has to do something, and Standard is the safe reading.
    fn from_page(page: Option<u32>) -> Self {
        match page {
            Some(1) => LaunchTab::Advanced,
            Some(2) => LaunchTab::Headless,
            _ => LaunchTab::Standard,
        }
    }

    /// The tab's label, which is also what the launch button offers.
    pub fn label(self) -> &'static str {
        match self {
            LaunchTab::Standard => crate::tr_en!("Standard"),
            LaunchTab::Advanced => crate::tr_en!("Advanced"),
            LaunchTab::Headless => crate::tr_en!("Headless"),
        }
    }
}

pub struct LaunchFormView {
    pub container: gtk::Box,
    services: Arc<AppServices>,
    type_combo: gtk::DropDown,
    registry_combo: gtk::DropDown,
    project_combo: gtk::DropDown,
    image_combo: gtk::DropDown,
    name_entry: adw::EntryRow,
    resource_type_switch: gtk::Switch,
    resource_selector: ResourceSelector,
    images: Rc<RefCell<Vec<ParsedImage>>>,
    launch_btn: gtk::Button,
    /// The status + launch row. Held so a modal host can pin it below its
    /// scroller instead of letting the form's primary action scroll away.
    action_row: gtk::Box,
    /// The form, or the result of submitting it. See `show_result`.
    body_stack: gtk::Stack,
    result_icon: gtk::Image,
    result_title: gtk::Label,
    result_detail: gtk::Label,
    result_back_btn: gtk::Button,
    /// The card's heading. A dialog supplies its own title, so a host that has
    /// one hides this rather than showing "Launch Session" twice.
    card_header: gtk::Box,
    status_label: gtk::Label,
    #[allow(clippy::type_complexity)]
    on_launched: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    session_limit_reached: Rc<RefCell<bool>>,
    active_sessions: Rc<RefCell<Vec<Session>>>,
    // Advanced tab
    custom_image_entry: adw::EntryRow,
    /// Full image URI chosen via the "Find images by package…" discovery
    /// dialog. When set and still matching the custom image entry text, the
    /// launch path uses it verbatim (it already includes the registry host)
    /// instead of prepending the advanced registry combo host.
    picked_image: Rc<RefCell<Option<String>>>,
    custom_type_combo: gtk::DropDown,
    /// Editable session-name field for the Advanced tab (the reference gives the
    /// Advanced pivot its own Name box + generate button rather than sharing the
    /// Standard tab's field).
    adv_name_entry: adw::EntryRow,
    adv_registry_combo: gtk::DropDown,
    registry_user_entry: adw::EntryRow,
    registry_secret_entry: adw::PasswordEntryRow,
    notebook: gtk::Notebook,
    // Headless (batch job) tab
    headless_image_entry: adw::EntryRow,
    headless_name_entry: adw::EntryRow,
    headless_cmd_entry: adw::EntryRow,
    headless_args_entry: adw::EntryRow,
    headless_replicas: gtk::SpinButton,
    /// Flexible (off) → send cores/ram/gpus = 0 (platform allocates); Fixed (on)
    /// → use `headless_resource_selector`. Mirrors HeadlessResourceType.
    headless_resource_type_switch: gtk::Switch,
    headless_resource_selector: ResourceSelector,
    headless_launch_btn: gtk::Button,
}

/// The session type to switch to in order to reach `image` in the Standard
/// tab, or `None` when the form offers none of the image's types.
///
/// Skaha advertises types the Standard tab does not launch — `headless` most
/// of all — so an image can be in the catalogue and still be unreachable
/// there. That is the case `select_image_by_id` hands to the Advanced tab,
/// and it is the only part of the decision that does not need a live GTK
/// dropdown, so it lives out here where it can be tested.
fn reachable_type(image: &ParsedImage) -> Option<&String> {
    image
        .types
        .iter()
        .find(|t| INTERACTIVE_SESSION_TYPES.contains(&t.as_str()))
}

impl LaunchFormView {
    pub fn new(services: Arc<AppServices>, active_sessions: Rc<RefCell<Vec<Session>>>) -> Rc<Self> {
        // The eighth Portal card, and the one that hand-rolled its own header
        // with no margins at all — which is why "Launch Session" sat flush
        // against the frame while the other seven titles were inset.
        let card = crate::ui::card::Card::new(crate::tr_en!("Launch Session"));
        let container = card.widget.clone();
        // Hug natural content height instead of stretching to fill the grid
        // row (which can be taller due to a sibling column), which was
        // opening a dead gap between the last form group and the bottom
        // Launch button row.
        container.set_valign(gtk::Align::Start);
        container.set_vexpand(false);
        card.content.set_vexpand(false);

        // Tabs: Standard / Advanced
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(false);
        notebook.set_valign(gtk::Align::Start);
        // No frame of its own. Whatever holds this form — the dialog, or a
        // card — is already a box; a notebook drawing a second one around the
        // tabs and every field under them is the border with nothing on the
        // other side of it. See `.flat-tabs` in style.css.
        notebook.add_css_class("flat-tabs");

        // === Standard Tab ===
        let standard_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        standard_box.set_margin_top(12);
        standard_box.set_margin_bottom(12);
        standard_box.set_valign(gtk::Align::Start);

        let form_group = adw::PreferencesGroup::new();

        // Session type
        let types_list = gtk::StringList::new(&INTERACTIVE_SESSION_TYPES);
        let type_combo = gtk::DropDown::new(Some(types_list), gtk::Expression::NONE);
        // Pre-select default session type from settings
        {
            let default_type = services.endpoints.config().default_session_type.clone();
            let idx = INTERACTIVE_SESSION_TYPES
                .iter()
                .position(|t| *t == default_type)
                .unwrap_or(0);
            type_combo.set_selected(idx as u32);
        }
        let type_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Session Type"))
            .build();
        // One width for every value in this group. Content-sized dropdowns have
        // their right edges aligned and their left edges anywhere, which reads
        // as ragged even though each row is individually correct.
        //
        // hexpand(false) matters as much as the width: a size_request is only a
        // MINIMUM, and the row's suffix box hands a greedy child every spare
        // pixel — so the four grew to roughly twice this and pushed "Container
        // Image" onto two lines.
        type_combo.set_size_request(crate::ui::space::FIELD, -1);
        type_combo.set_hexpand(false);
        type_combo.set_halign(gtk::Align::End);
        type_row.add_suffix(&type_combo);
        form_group.add(&type_row);

        // Image Registry
        let registry_list = gtk::StringList::new(&[]);
        let registry_combo = gtk::DropDown::new(Some(registry_list), gtk::Expression::NONE);
        let registry_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Image Registry"))
            .build();
        registry_combo.set_size_request(crate::ui::space::FIELD, -1);
        registry_combo.set_hexpand(false);
        registry_combo.set_halign(gtk::Align::End);
        registry_row.add_suffix(&registry_combo);
        form_group.add(&registry_row);

        // Project
        let project_list = gtk::StringList::new(&[]);
        let project_combo = gtk::DropDown::new(Some(project_list), gtk::Expression::NONE);
        let project_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Project"))
            .build();
        project_combo.set_size_request(crate::ui::space::FIELD, -1);
        project_combo.set_hexpand(false);
        project_combo.set_halign(gtk::Align::End);
        project_row.add_suffix(&project_combo);
        form_group.add(&project_row);

        // Image
        let image_list = gtk::StringList::new(&[]);
        let image_combo = gtk::DropDown::new(Some(image_list), gtk::Expression::NONE);
        let image_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Container Image"))
            .build();
        // Discovery: search images by installed package/capability.
        let find_images_btn = gtk::Button::from_icon_name("system-search-symbolic");
        find_images_btn.set_tooltip_text(Some(crate::tr_en!("Find images by package…")));
        find_images_btn.add_css_class("flat");
        find_images_btn.set_valign(gtk::Align::Center);
        image_row.add_suffix(&find_images_btn);
        image_combo.set_size_request(crate::ui::space::FIELD, -1);
        image_combo.set_hexpand(false);
        image_combo.set_halign(gtk::Align::End);
        image_row.add_suffix(&image_combo);
        form_group.add(&image_row);

        // Session name
        let name_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Session Name"))
            .build();
        // Manual "Generate name" action: re-derives the auto session name from
        // the currently selected type (mirrors the reference's GenerateSessionName).
        let generate_name_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        generate_name_btn.set_tooltip_text(Some(crate::tr_en!("Generate name")));
        generate_name_btn.add_css_class("flat");
        generate_name_btn.set_valign(gtk::Align::Center);
        name_entry.add_suffix(&generate_name_btn);
        form_group.add(&name_entry);

        standard_box.append(&form_group);

        // Resource type toggle
        let resource_group = adw::PreferencesGroup::new();
        let resource_type_switch = gtk::Switch::new();
        resource_type_switch.set_active(false);
        resource_type_switch.set_valign(gtk::Align::Center);

        let resource_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Fixed Resources"))
            .subtitle(crate::tr_en!("Enable to specify exact CPU/RAM/GPU"))
            .build();
        resource_row.add_suffix(&resource_type_switch);
        resource_row.set_activatable_widget(Some(&resource_type_switch));
        resource_group.add(&resource_row);
        standard_box.append(&resource_group);

        // Resource selector
        let resource_selector = ResourceSelector::new();
        resource_selector.widget().set_visible(false);
        standard_box.append(resource_selector.widget());

        notebook.append_page(
            &standard_box,
            Some(&gtk::Label::new(Some(crate::tr_en!("Standard")))),
        );

        // === Advanced Tab ===
        let advanced_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        advanced_box.set_margin_top(12);
        advanced_box.set_margin_bottom(12);
        advanced_box.set_valign(gtk::Align::Start);

        let adv_group = adw::PreferencesGroup::builder()
            .title(crate::tr_en!("Custom Container Image"))
            .description(crate::tr_en!("Launch a session using a custom image URI"))
            .build();

        // Session type
        let custom_type_list = gtk::StringList::new(&LAUNCHABLE_SESSION_TYPES);
        let custom_type_combo = gtk::DropDown::new(Some(custom_type_list), gtk::Expression::NONE);
        let custom_type_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Session Type"))
            .build();
        custom_type_row.add_suffix(&custom_type_combo);
        adv_group.add(&custom_type_row);

        // Image Registry (from API repositories)
        let adv_registry_list = gtk::StringList::new(&[]);
        let adv_registry_combo = gtk::DropDown::new(Some(adv_registry_list), gtk::Expression::NONE);
        let adv_registry_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Image Registry"))
            .build();
        adv_registry_row.add_suffix(&adv_registry_combo);
        adv_group.add(&adv_registry_row);

        // Custom image URI (project/name:tag)
        let custom_image_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Image (project/name:tag)"))
            .build();
        adv_group.add(&custom_image_entry);

        // Session name (Advanced tab has its own editable name + generate button).
        let adv_name_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Session Name"))
            .build();
        let adv_generate_name_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        adv_generate_name_btn.set_tooltip_text(Some(crate::tr_en!("Generate name")));
        adv_generate_name_btn.add_css_class("flat");
        adv_generate_name_btn.set_valign(gtk::Align::Center);
        adv_name_entry.add_suffix(&adv_generate_name_btn);
        adv_group.add(&adv_name_entry);

        // Registry auth
        let auth_group = adw::PreferencesGroup::builder()
            .title(crate::tr_en!("Registry Authentication"))
            .description(crate::tr_en!(
                "Credentials for private registries. Leave blank for public images."
            ))
            .build();

        let registry_user_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Username"))
            .build();
        auth_group.add(&registry_user_entry);

        let registry_secret_entry = adw::PasswordEntryRow::builder()
            .title(crate::tr_en!("Token or Password"))
            .build();
        auth_group.add(&registry_secret_entry);

        advanced_box.append(&adv_group);
        advanced_box.append(&auth_group);
        notebook.append_page(
            &advanced_box,
            Some(&gtk::Label::new(Some(crate::tr_en!("Advanced")))),
        );

        // === Headless (batch job) Tab ===
        let headless_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        headless_box.set_margin_top(12);
        headless_box.set_margin_bottom(12);
        headless_box.set_valign(gtk::Align::Start);

        let headless_group = adw::PreferencesGroup::new();
        headless_group.set_title(crate::tr_en!("Headless Batch Job"));
        headless_group.set_description(Some(crate::tr_en!(
            "Run a container command with no interactive UI. Replicas launch the same job N times."
        )));

        let headless_image_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Container Image"))
            .build();
        headless_group.add(&headless_image_entry);
        let headless_name_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Job Name"))
            .build();
        let headless_generate_name_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        headless_generate_name_btn.set_tooltip_text(Some(crate::tr_en!("Generate name")));
        headless_generate_name_btn.add_css_class("flat");
        headless_generate_name_btn.set_valign(gtk::Align::Center);
        headless_name_entry.add_suffix(&headless_generate_name_btn);
        headless_group.add(&headless_name_entry);
        let headless_cmd_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Command"))
            .build();
        headless_group.add(&headless_cmd_entry);
        let headless_args_entry = adw::EntryRow::builder()
            .title(crate::tr_en!("Arguments (space-separated)"))
            .build();
        headless_group.add(&headless_args_entry);

        // The same range `launch_headless_job` advertises — a spinner that
        // stopped short would silently rewrite a count an agent had validated.
        let (replicas_lo, replicas_hi) = crate::models::session_launch_params::REPLICAS_RANGE;
        let headless_replicas =
            gtk::SpinButton::with_range(replicas_lo as f64, replicas_hi as f64, 1.0);
        headless_replicas.set_value(1.0);
        headless_replicas.set_valign(gtk::Align::Center);
        let replicas_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Replicas"))
            .subtitle(crate::tr_fmt!(
                "{}–{} identical jobs",
                replicas_lo,
                replicas_hi
            ))
            .build();
        replicas_row.add_suffix(&headless_replicas);
        headless_group.add(&replicas_row);

        headless_box.append(&headless_group);

        // Resource type: Flexible (platform-managed) vs Fixed. Flexible sends
        // cores/ram/gpus = 0; Fixed reveals a full ResourceSelector. Mirrors the
        // Headless PivotItem's Flexible/Fixed RadioButtons + HeadlessResourcePanel.
        let headless_resource_group = adw::PreferencesGroup::new();
        let headless_resource_type_switch = gtk::Switch::new();
        headless_resource_type_switch.set_active(false);
        headless_resource_type_switch.set_valign(gtk::Align::Center);
        let headless_resource_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Fixed Resources"))
            .subtitle(crate::tr_en!(
                "Off: flexible (platform-managed). On: specify exact CPU/RAM/GPU."
            ))
            .build();
        headless_resource_row.add_suffix(&headless_resource_type_switch);
        headless_resource_row.set_activatable_widget(Some(&headless_resource_type_switch));
        headless_resource_group.add(&headless_resource_row);
        headless_box.append(&headless_resource_group);

        let headless_resource_selector = ResourceSelector::new();
        headless_resource_selector.widget().set_visible(false);
        headless_box.append(headless_resource_selector.widget());

        // Built here with the rest of the Headless tab, but PARENTED in the
        // shared bottom row below — see there for why.
        let headless_launch_btn = gtk::Button::with_label(crate::tr_en!("Launch Job"));
        headless_launch_btn.add_css_class("suggested-action");

        notebook.append_page(
            &headless_box,
            Some(&gtk::Label::new(Some(crate::tr_en!("Headless")))),
        );

        // The form, or the result of submitting it — never both, and never a
        // second window on top of this one. A launch used to raise its own
        // dialog over the modal, which is a modal on a modal: the pattern that
        // froze this app once, and two things to dismiss for one action.
        //
        // A Stack rather than toggled visibility, for the crossfade. This is a
        // content swap in a modal the user is looking straight at — one of the
        // few places in this app where motion earns its keep, because an
        // instant substitution reads as the window having been replaced.
        let body_stack = gtk::Stack::new();
        body_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        body_stack.set_transition_duration(RESULT_FADE_MS);
        body_stack.add_named(&notebook, Some(BODY_FORM));

        let result_panel = gtk::Box::new(gtk::Orientation::Vertical, crate::ui::space::CARD);
        result_panel.set_valign(gtk::Align::Center);
        result_panel.set_halign(gtk::Align::Center);
        result_panel.set_vexpand(true);
        crate::ui::space::edge_all(&result_panel);

        let result_icon = gtk::Image::from_icon_name("emblem-ok-symbolic");
        result_icon.set_pixel_size(RESULT_ICON_PX);
        result_panel.append(&result_icon);

        let result_title = gtk::Label::new(None);
        result_title.add_css_class("title-2");
        result_title.set_wrap(true);
        result_title.set_justify(gtk::Justification::Center);
        result_panel.append(&result_title);

        let result_detail = gtk::Label::new(None);
        result_detail.add_css_class("dim-label");
        result_detail.set_wrap(true);
        result_detail.set_justify(gtk::Justification::Center);
        result_panel.append(&result_detail);

        // Only for a failure. A success closes itself, so a button there would
        // be one nobody can press in time.
        let result_back_btn = gtk::Button::with_label(crate::tr_en!("Back to the form"));
        result_back_btn.set_halign(gtk::Align::Center);
        result_back_btn.set_visible(false);
        result_panel.append(&result_back_btn);

        body_stack.add_named(&result_panel, Some(BODY_RESULT));
        card.content.append(&body_stack);

        // Status + Launch button
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_hexpand(true);
        status_label.set_halign(gtk::Align::Start);
        bottom.append(&status_label);

        // Both launch buttons live in the one row, and `sync_launch_button`
        // shows whichever belongs to the visible tab.
        //
        // "Launch Job" used to sit at the bottom of the Headless PAGE, inside
        // the scrolling area. In a modal that puts it below the fold — the
        // form is taller than the dialog — so the primary action of the tab was
        // reachable only by scrolling to it. One row, pinned by the host, is
        // also one place to look for "how do I start this".
        let launch_btn = gtk::Button::with_label(crate::tr_en!("Launch"));
        launch_btn.add_css_class("suggested-action");
        bottom.append(&launch_btn);
        bottom.append(&headless_launch_btn);

        card.content.append(&bottom);

        // Toggle resource selector visibility
        {
            let resource_widget = resource_selector.widget().clone();
            resource_type_switch.connect_active_notify(move |switch| {
                resource_widget.set_visible(switch.is_active());
            });
        }

        // Toggle headless resource selector visibility (Fixed → visible).
        {
            let resource_widget = headless_resource_selector.widget().clone();
            headless_resource_type_switch.connect_active_notify(move |switch| {
                resource_widget.set_visible(switch.is_active());
            });
        }

        let card_header = card.header.clone();
        let view = Rc::new(LaunchFormView {
            container,
            services,
            type_combo,
            registry_combo,
            project_combo,
            image_combo,
            name_entry,
            resource_type_switch,
            resource_selector,
            images: Rc::new(RefCell::new(Vec::new())),
            launch_btn,
            action_row: bottom,
            body_stack,
            result_icon,
            result_title,
            result_detail,
            result_back_btn,
            card_header,
            status_label,
            on_launched: Rc::new(RefCell::new(None)),
            session_limit_reached: Rc::new(RefCell::new(false)),
            active_sessions,
            custom_image_entry,
            picked_image: Rc::new(RefCell::new(None)),
            custom_type_combo,
            adv_name_entry,
            adv_registry_combo,
            registry_user_entry,
            registry_secret_entry,
            notebook,
            headless_image_entry,
            headless_name_entry,
            headless_cmd_entry,
            headless_args_entry,
            headless_replicas,
            headless_resource_type_switch,
            headless_resource_selector,
            headless_launch_btn,
        });

        // Type change -> update registries
        {
            let view_clone = view.clone();
            let type_combo = view.type_combo.clone();
            type_combo.connect_selected_notify(move |_| {
                view_clone.update_registries();
            });
        }

        // Tab change -> the shared Launch button follows the visible tab.
        //
        // Wired as well as called from `show_tab`, because the user can switch
        // tabs inside the modal and the button must not be left offering a
        // Standard launch over the Headless form.
        {
            let view_clone = view.clone();
            view.notebook.connect_switch_page(move |_, _, page| {
                view_clone.sync_launch_button(LaunchTab::from_page(Some(page)));
            });
        }

        // Registry change -> update projects
        {
            let view_clone = view.clone();
            let registry_combo = view.registry_combo.clone();
            registry_combo.connect_selected_notify(move |_| {
                view_clone.update_projects();
            });
        }

        // Project change -> update images
        {
            let view_clone = view.clone();
            let project_combo = view.project_combo.clone();
            project_combo.connect_selected_notify(move |_| {
                view_clone.update_images();
            });
        }

        // Advanced type change -> update name
        {
            let view_clone = view.clone();
            let custom_type_combo = view.custom_type_combo.clone();
            custom_type_combo.connect_selected_notify(move |_| {
                view_clone.update_advanced_name();
            });
        }

        // "Find images by package…" -> open the image discovery dialog. The
        // picked full image URI is injected into the launch path via
        // `apply_picked_image` (switches to the Advanced tab).
        {
            let view_clone = view.clone();
            find_images_btn.connect_clicked(move |btn| {
                let services = view_clone.services.clone();
                let view_for_pick = view_clone.clone();
                let on_pick: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
                    view_for_pick.apply_picked_image(&id);
                });
                crate::ui::image_discovery_dialog::show_image_discovery_dialog(
                    btn, services, on_pick,
                );
            });
        }

        // Manual "Generate name" button next to the session-name field
        {
            let view_clone = view.clone();
            generate_name_btn.connect_clicked(move |_| {
                view_clone.generate_session_name();
            });
        }

        // Advanced tab "Generate name" button (writes the Advanced name field).
        {
            let view_clone = view.clone();
            adv_generate_name_btn.connect_clicked(move |_| {
                view_clone.update_advanced_name();
            });
        }

        // Headless tab "Generate name" button (writes `headless<n+1>`).
        {
            let view_clone = view.clone();
            headless_generate_name_btn.connect_clicked(move |_| {
                view_clone.generate_headless_name();
            });
        }

        // Save as template button

        // Launch button
        {
            let view_clone = view.clone();
            let launch_btn = view.launch_btn.clone();
            launch_btn.connect_clicked(move |_| {
                let view_clone = view_clone.clone();
                glib::spawn_future_local(async move {
                    view_clone.do_launch().await;
                });
            });
        }

        // Headless "Launch Job" button
        {
            let view_clone = view.clone();
            let btn = view.headless_launch_btn.clone();
            btn.connect_clicked(move |_| {
                let view_clone = view_clone.clone();
                glib::spawn_future_local(async move {
                    view_clone.do_launch_headless().await;
                });
            });
        }

        // The way back from a failure. Nothing else returns to the form: a
        // success closes the modal, and closing it resets the page anyway.
        {
            let back = view.result_back_btn.clone();
            let form = view.clone();
            back.connect_clicked(move |_| form.clear_result());
        }

        view
    }

    /// Show `tab`. The floating launch button opens the form on the one the
    /// user picked; `select_image_by_id` uses it to land on the tab that can
    /// actually express the image it was given.
    /// The status + launch row, for a host that wants to pin it.
    ///
    /// Reparenting it is the host's business; `restore_action_row` puts it back.
    pub fn action_row(&self) -> &gtk::Box {
        &self.action_row
    }

    /// Put the action row back under the form, where it lives when nothing has
    /// borrowed it.
    pub fn restore_action_row(&self) {
        if self.action_row.parent().is_none() {
            self.card_content_append(&self.action_row);
        }
    }

    /// Show or hide the card's own heading.
    ///
    /// A dialog already has a title bar; leaving this on prints "Launch
    /// Session" twice, once in the header bar and once inside the card.
    pub fn set_header_visible(&self, visible: bool) {
        self.card_header.set_visible(visible);
    }

    /// Put the result of a launch where the form was.
    ///
    /// Replaces the dialog that used to open on top of this one. Two windows to
    /// dismiss for one action was the least of it: a modal raised from a modal
    /// is the arrangement that left this app frozen behind its own dialog.
    fn show_result(&self, ok: bool, title: &str, detail: &str) {
        self.result_icon
            .set_icon_name(Some(if ok { "emblem-ok-symbolic" } else { "dialog-error-symbolic" }));
        // Semantic colour, not the accent: this says what happened, and it has
        // to read as the same thing here as it does on a session card.
        self.result_icon.remove_css_class("success");
        self.result_icon.remove_css_class("error");
        self.result_icon.add_css_class(if ok { "success" } else { "error" });

        self.result_title.set_text(title);
        self.result_detail.set_text(detail);
        // A success closes itself, so a button there is one nobody can press in
        // time. A failure stays, and needs the way back.
        self.result_back_btn.set_visible(!ok);

        // The launch controls belong to the form; on the result page they would
        // offer to do again what has just been done.
        self.launch_btn.set_visible(false);
        self.headless_launch_btn.set_visible(false);
        self.status_label.set_text("");
        self.body_stack.set_visible_child_name(BODY_RESULT);
    }

    /// Back to the form.
    pub fn clear_result(&self) {
        self.body_stack.set_visible_child_name(BODY_FORM);
        self.result_back_btn.set_visible(false);
        self.sync_launch_button(self.current_tab());
    }

    /// Show or hide the card's frame.
    ///
    /// On the Portal this is one card among seven and the frame is what makes
    /// it one. In a dialog it is the only thing there, so the frame draws a
    /// second box a few pixels inside the first.
    pub fn set_framed(&self, framed: bool) {
        crate::ui::card::set_framed(&self.container, framed);
    }

    /// The card's content box — the action row's home.
    fn card_content_append(&self, child: &impl IsA<gtk::Widget>) {
        // `container` is the card; its content box is the last child, the one
        // the header is not.
        if let Some(content) = self.container.last_child().and_downcast::<gtk::Box>() {
            content.append(child);
        }
    }

    pub fn show_tab(&self, tab: LaunchTab) {
        self.notebook.set_current_page(Some(tab.page()));
        self.sync_launch_button(tab);
    }

    /// The tab currently showing.
    pub fn current_tab(&self) -> LaunchTab {
        LaunchTab::from_page(self.notebook.current_page())
    }

    /// Keep the shared bottom Launch button honest about the visible tab.
    ///
    /// Standard and Advanced share it; Headless has its own "Launch Job" button
    /// and its own launch path. The shared button used to stay visible and
    /// enabled on the Headless tab while `do_launch` — which only asked whether
    /// the page was Advanced — ran the STANDARD launch. Nobody hit it often
    /// because reaching Headless took two deliberate clicks; opening the form
    /// directly on Headless makes that button the nearest thing to hand.
    fn sync_launch_button(&self, tab: LaunchTab) {
        let headless = tab == LaunchTab::Headless;
        self.launch_btn.set_visible(!headless);
        self.headless_launch_btn.set_visible(headless);
    }

    pub fn set_on_launched(&self, callback: impl Fn() + 'static) {
        *self.on_launched.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_session_limit_reached(&self, reached: bool) {
        *self.session_limit_reached.borrow_mut() = reached;
        if reached {
            self.launch_btn.set_sensitive(false);
            self.status_label.set_text(crate::tr_en!(
                "Session limit reached (max 3 concurrent sessions)"
            ));
        } else {
            self.launch_btn.set_sensitive(true);
            self.status_label.set_text("");
        }
    }

    pub async fn load_images(&self) {
        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let Some(token) = token else {
                    return Err("Not authenticated".to_string());
                };
                // The merged catalogue: an image the user added from the
                // registry is launchable, so it belongs in the picker too.
                let images = svc.image_catalogue(&token).await?;
                let context = svc.images.get_context(&token).await.ok();
                let repos = svc
                    .images
                    .get_repositories(&token)
                    .await
                    .unwrap_or_default();
                Ok((images, context, repos))
            })
            .await;

        match result {
            Ok((parsed, context, repos)) => {
                *self.images.borrow_mut() = parsed;
                self.update_registries();

                // Populate advanced registry combo from API repositories
                let adv_model = gtk::StringList::new(&[]);
                for r in &repos {
                    adv_model.append(r);
                }
                self.adv_registry_combo.set_model(Some(&adv_model));
                if !repos.is_empty() {
                    self.adv_registry_combo.set_selected(0);
                }

                if let Some(context) = context {
                    let core_opts = context.core_options();
                    let mem_opts = context.memory_options();
                    let gpu_opts = context.gpu_options();
                    self.resource_selector
                        .set_core_options(&core_opts, context.default_cores());
                    self.resource_selector
                        .set_memory_options(&mem_opts, context.default_memory());
                    self.resource_selector.set_gpu_options(&gpu_opts);
                    // The headless Fixed selector uses the same platform context.
                    self.headless_resource_selector
                        .set_core_options(&core_opts, context.default_cores());
                    self.headless_resource_selector
                        .set_memory_options(&mem_opts, context.default_memory());
                    self.headless_resource_selector.set_gpu_options(&gpu_opts);
                }

                // Seed the Advanced and Headless tab name fields (they have their
                // own editable Name boxes; mirrors GenerateSessionName +
                // GenerateHeadlessSessionName in LoadImagesAndContextAsync).
                self.update_advanced_name();
                self.generate_headless_name();
            }
            Err(e) => {
                self.status_label
                    .set_text(&crate::tr_fmt!("Failed to load images: {}", e));
            }
        }
    }

    /// The session type the user picked, read from the dropdown's OWN model.
    ///
    /// Not by indexing a parallel array: that copy has to be kept in step with
    /// the one the combo was built from, and nothing enforces it — a type added
    /// to the dropdown alone would have launched whatever sat at that index in
    /// the stale list.
    fn selected_type(&self) -> String {
        let picked = self.combo_selected_string(&self.type_combo);
        if picked.is_empty() {
            INTERACTIVE_SESSION_TYPES[0].to_string()
        } else {
            picked
        }
    }

    fn selected_registry(&self) -> String {
        self.combo_selected_string(&self.registry_combo)
    }

    fn combo_selected_string(&self, combo: &gtk::DropDown) -> String {
        combo
            .model()
            .and_then(|m| {
                m.downcast_ref::<gtk::StringList>()
                    .map(|sl| sl.string(combo.selected()).map(|s| s.to_string()))
            })
            .flatten()
            .unwrap_or_default()
    }

    fn session_count_for_type(&self, session_type: &str) -> usize {
        self.active_sessions
            .borrow()
            .iter()
            .filter(|s| s.session_type.eq_ignore_ascii_case(session_type))
            .count()
    }

    fn default_image_for_type(session_type: &str) -> Option<&'static str> {
        match session_type {
            "notebook" => Some("astroml:latest"),
            "desktop" => Some("desktop:latest"),
            "carta" => Some("carta:latest"),
            "contributed" => Some("astroml-vscode:latest"),
            "firefly" => Some("firefly:2025.2"),
            _ => None,
        }
    }

    fn update_registries(&self) {
        let session_type = self.selected_type();
        let images = self.images.borrow();
        let registries = ImageParser::registries_for_type(&images, &session_type);

        let model = gtk::StringList::new(&[]);
        for r in &registries {
            model.append(r);
        }
        self.registry_combo.set_model(Some(&model));
        if !registries.is_empty() {
            self.registry_combo.set_selected(0);
        }
        self.update_projects();
    }

    fn update_projects(&self) {
        let session_type = self.selected_type();
        let registry = self.selected_registry();
        let images = self.images.borrow();
        let projects =
            ImageParser::projects_for_type_and_registry(&images, &session_type, &registry);

        let model = gtk::StringList::new(&[]);
        for p in &projects {
            model.append(p);
        }
        self.project_combo.set_model(Some(&model));

        // Prefer the project that contains the default image for this type
        let mut selected_idx = 0;
        if let Some(default_name) = Self::default_image_for_type(&session_type) {
            for (i, project) in projects.iter().enumerate() {
                let proj_images = ImageParser::images_for_type_registry_and_project(
                    &images,
                    &session_type,
                    &registry,
                    project,
                );
                if proj_images
                    .iter()
                    .any(|img| img.id.ends_with(default_name) || img.display_name == default_name)
                {
                    selected_idx = i;
                    break;
                }
            }
        }
        if !projects.is_empty() {
            self.project_combo.set_selected(selected_idx as u32);
        }
        self.update_images();
    }

    fn update_images(&self) {
        let session_type = self.selected_type();
        let filtered = self.visible_images();

        let model = gtk::StringList::new(&[]);
        for img in &filtered {
            model.append(&img.display_name);
        }
        self.image_combo.set_model(Some(&model));

        // Try to select the default image for this type
        let mut selected_idx = 0;
        if let Some(default_name) = Self::default_image_for_type(&session_type) {
            for (i, img) in filtered.iter().enumerate() {
                if img.id.ends_with(default_name) || img.display_name == default_name {
                    selected_idx = i;
                    break;
                }
            }
        }
        if !filtered.is_empty() {
            self.image_combo.set_selected(selected_idx as u32);
        }

        // Auto-generate session name: type + (count of that type + 1)
        let count = self.session_count_for_type(&session_type);
        let name =
            crate::models::session_launch_params::numbered_session_name(&session_type, count);
        self.name_entry.set_text(&name);
    }

    fn get_selected_image_id(&self) -> Option<String> {
        let idx = self.image_combo.selected() as usize;
        self.visible_images().get(idx).map(|img| img.id.clone())
    }

    /// The Advanced tab's session type, decoded from the list the dropdown was
    /// BUILT from.
    ///
    /// Three call sites each kept their own copy of that list. They agreed
    /// today; the day a type is added to `LAUNCHABLE_SESSION_TYPES` alone, every
    /// selection past it decodes to the wrong type — the user picks CARTA and
    /// launches a desktop, with nothing to indicate it.
    fn advanced_session_type(&self) -> &'static str {
        let idx = self.custom_type_combo.selected() as usize;
        LAUNCHABLE_SESSION_TYPES
            .get(idx)
            .copied()
            .unwrap_or(LAUNCHABLE_SESSION_TYPES[0])
    }

    fn update_advanced_name(&self) {
        let session_type = self.advanced_session_type();
        let count = self.session_count_for_type(session_type);
        let name = crate::models::session_launch_params::numbered_session_name(session_type, count);
        self.adv_name_entry.set_text(&name);
    }

    /// Re-derive the headless job name (`headless<count+1>`), wired to the
    /// Headless tab's "Generate name" button. Mirrors GenerateHeadlessSessionName.
    fn generate_headless_name(&self) {
        let count = self.session_count_for_type("headless");
        self.headless_name_entry.set_text(
            &crate::models::session_launch_params::numbered_session_name("headless", count),
        );
    }

    /// Re-derive the Standard-tab session name from its selected type
    /// (`<type><count+1>`), wired to the Standard tab's "Generate name" button.
    /// The Advanced and Headless tabs have their own generate buttons writing
    /// their own name fields (`update_advanced_name` / `generate_headless_name`).
    fn generate_session_name(&self) {
        let session_type = self.selected_type();
        let count = self.session_count_for_type(&session_type);
        self.name_entry.set_text(
            &crate::models::session_launch_params::numbered_session_name(&session_type, count),
        );
    }

    /// Inject an image chosen via the discovery dialog into the launch path.
    /// The picked id is a fully-qualified URI (e.g. `images.canfar.net/…:tag`),
    /// so it is placed into the Advanced tab's custom image entry (which the
    /// launch path reads) and the Advanced tab is brought to the front.
    /// Select `image_id` in the Standard tab, or fall back to the Advanced
    /// tab's custom-image field when it is not a launchable catalogue image.
    ///
    /// Port of `SessionLaunchViewModel.SelectImageById`, including that
    /// fallback: an image the discovery cache knows about is not necessarily
    /// one Skaha will offer for a session type, and dropping the request on the
    /// floor is how "Use this image" came to do nothing at all.
    ///
    /// Returns whether it landed in the catalogue.
    pub fn select_image_by_id(&self, image_id: &str) -> bool {
        let image_id = image_id.trim();
        if image_id.is_empty() {
            return false;
        }

        // The combos filter the image list, so they have to be moved to where
        // the image actually is before it can be selected — setting the
        // dropdown index alone would pick whatever happened to sit there.
        let target = self
            .images
            .borrow()
            .iter()
            .find(|img| img.id == image_id)
            .cloned();

        if let Some(target) = target {
            let placed = self.point_combos_at(&target);
            if placed {
                self.update_images();
                if let Some(index) = self.visible_images().iter().position(|i| i.id == image_id) {
                    self.image_combo.set_selected(index as u32);
                    self.show_tab(LaunchTab::Standard);
                    self.status_label
                        .set_text(&crate::tr_fmt!("Selected image: {}", image_id));
                    return true;
                }
            }
        }

        // Not a launchable catalogue image — a private one only discovery knows
        // about, say. The Advanced tab takes a full URI, which is exactly what
        // we have.
        self.apply_picked_image(image_id);
        false
    }

    /// Move the type, registry and project combos to where `target` lives.
    /// False when any of them has no entry for it.
    fn point_combos_at(&self, target: &ParsedImage) -> bool {
        let Some(session_type) = reachable_type(target) else {
            return false;
        };
        let Some(type_index) = self.combo_index(&self.type_combo, session_type) else {
            return false;
        };
        self.type_combo.set_selected(type_index);

        if let Some(index) = self.combo_index(&self.registry_combo, &target.registry) {
            self.registry_combo.set_selected(index);
        }
        // Selecting a type repopulates the project list, so the project has to
        // be chosen after it.
        self.update_projects();
        if let Some(index) = self.combo_index(&self.project_combo, &target.project) {
            self.project_combo.set_selected(index);
        }
        true
    }

    /// The position of `value` in a dropdown's string model.
    fn combo_index(&self, combo: &gtk::DropDown, value: &str) -> Option<u32> {
        let model = combo.model()?.downcast::<gtk::StringList>().ok()?;
        (0..model.n_items()).find(|i| model.string(*i).map(|s| s == value).unwrap_or(false))
    }

    /// The images the Standard tab's dropdown currently lists, in its order.
    ///
    /// The filter is spelled out in three places — `update_images`,
    /// `get_selected_image_id` and here — so it lives here and they call it.
    fn visible_images(&self) -> Vec<ParsedImage> {
        ImageParser::images_for_type_registry_and_project(
            &self.images.borrow(),
            &self.selected_type(),
            &self.selected_registry(),
            &self.combo_selected_string(&self.project_combo),
        )
    }

    fn apply_picked_image(&self, id: &str) {
        *self.picked_image.borrow_mut() = Some(id.to_string());
        self.custom_image_entry.set_text(id);
        self.show_tab(LaunchTab::Advanced);
        self.status_label
            .set_text(&crate::tr_fmt!("Selected image: {}", id));
    }

    /// Resolve the container image URI for the Advanced tab. When the user
    /// picked a fully-qualified image via the discovery dialog and left it
    /// unedited, it is returned verbatim; otherwise the advanced registry host
    /// is prepended to the entered `project/name:tag` path.
    fn advanced_image_uri(&self) -> String {
        let custom_path = self.custom_image_entry.text().to_string();
        if let Some(picked) = self.picked_image.borrow().as_ref() {
            if *picked == custom_path {
                return custom_path;
            }
        }
        let registry_host = self.combo_selected_string(&self.adv_registry_combo);
        if registry_host.is_empty() {
            custom_path
        } else {
            format!("{}/{}", registry_host, custom_path)
        }
    }

    async fn do_launch(&self) {
        if *self.session_limit_reached.borrow() {
            return;
        }
        let task = crate::helpers::tasks::begin(
            crate::helpers::tasks::TaskKind::Launch,
            crate::tr_en!("Launch session"),
        );

        let is_advanced = self.current_tab() == LaunchTab::Advanced;

        let (session_type, image, reg_user, reg_secret) = if is_advanced {
            let st = self.advanced_session_type().to_string();
            // Resolve custom URI (honours a discovery-picked full image).
            let img = self.advanced_image_uri();
            let ru = {
                let text = self.registry_user_entry.text().to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            };
            let rs = {
                let text = self.registry_secret_entry.text().to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            };
            (st, img, ru, rs)
        } else {
            let st = self.selected_type();
            let img = match self.get_selected_image_id() {
                Some(id) => id,
                None => {
                    self.status_label
                        .set_text(crate::tr_en!("Please select an image"));
                    return;
                }
            };
            (st, img, None, None)
        };

        if image.is_empty() {
            self.status_label
                .set_text(crate::tr_en!("Please select or enter an image"));
            return;
        }

        // The Advanced tab has its own Name field; the Standard tab uses the
        // shared one.
        let name = if is_advanced {
            self.adv_name_entry.text().to_string()
        } else {
            self.name_entry.text().to_string()
        };
        if name.is_empty() {
            self.status_label
                .set_text(crate::tr_en!("Please enter a session name"));
            return;
        }

        let (cores, ram, gpus) = if self.resource_type_switch.is_active() {
            (
                self.resource_selector.cores(),
                self.resource_selector.ram(),
                self.resource_selector.gpus(),
            )
        } else {
            // Flexible resources: send 0 so the platform allocates (matches the
            // reference). Sending fixed defaults here would defeat flexible mode.
            (0, 0, 0)
        };

        // Snapshot resource mode + project before the async launch (the user may
        // edit the form while it is in flight) so the recent-launch record and a
        // later relaunch reproduce the same configuration.
        let resource_type = if self.resource_type_switch.is_active() {
            "fixed"
        } else {
            "flexible"
        }
        .to_string();
        let project = if is_advanced {
            None
        } else {
            let p = self.combo_selected_string(&self.project_combo);
            if p.is_empty() {
                None
            } else {
                Some(p)
            }
        };

        let params = SessionLaunchParams {
            name: name.clone(),
            image: image.clone(),
            session_type: session_type.clone(),
            cores,
            ram,
            gpus,
            cmd: None,
            env: None,
            registry_username: reg_user,
            registry_secret: reg_secret,
            args: None,
            replicas: None,
        };

        self.launch_btn.set_sensitive(false);
        self.status_label
            .set_text(crate::tr_en!("Launching session..."));

        task.stage(crate::tr_fmt!("submitting {}", params.name));
        let svc = self.services.clone();
        let params_clone = params.clone();
        let launch_result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let Some(token) = token else {
                    return Err("Not authenticated".to_string());
                };
                svc.sessions.launch_session(&token, &params_clone).await
            })
            .await;

        // Display short image name for dialog
        let image_display = match image.rsplit_once('/') {
            Some((_, tail)) => tail.to_string(),
            None => image.clone(),
        };

        match &launch_result {
            Ok(_) => task.succeed(),
            Err(e) => task.fail(e.clone()),
        }

        // Shown where the form was, not in a window on top of it.
        let mut summary = vec![format!("CPU {cores}"), format!("RAM {ram}G")];
        if gpus > 0 {
            summary.push(format!("GPU {gpus}"));
        }
        let detail = format!(
            "{}  \u{00B7}  {}  \u{00B7}  {}",
            image_display,
            session_type,
            summary.join("  \u{00B7}  ")
        );

        match &launch_result {
            Ok(_) => self.show_result(
                true,
                &crate::tr_fmt!("{} is starting", name),
                &detail,
            ),
            Err(e) => self.show_result(
                false,
                crate::tr_en!("The session could not be started"),
                e,
            ),
        }

        match launch_result {
            Ok(_) => {
                self.status_label.set_text("");

                // Save to recent launches
                let now = chrono::Local::now().to_rfc3339();
                let recent = RecentLaunch {
                    name,
                    session_type,
                    image,
                    cores,
                    ram,
                    gpus,
                    timestamp: now.clone(),
                    project,
                    resource_type: Some(resource_type),
                    cmd: None,
                    args: None,
                    replicas: None,
                    launched_at: Some(now),
                };
                let _ = self.services.recent_launches.save(recent);

                // Let the confirmation be read, then let the host close the
                // modal. The dwell is here rather than in the host because it
                // is the confirmation's own timing — the host only knows that
                // the launch is done.
                glib::timeout_future(std::time::Duration::from_millis(RESULT_DWELL_MS as u64))
                    .await;
                if let Some(ref cb) = *self.on_launched.borrow() {
                    cb();
                }
            }
            Err(e) => {
                // The result panel carries the reason; the status line under a
                // form nobody is looking at would be saying it to the wall.
                let _ = e;
            }
        }

        self.launch_btn.set_sensitive(true);
    }

    /// Launch a headless (batch) job from the Headless tab.
    async fn do_launch_headless(&self) {
        let image = self.headless_image_entry.text().to_string();
        if image.is_empty() {
            self.status_label
                .set_text(crate::tr_en!("Please enter a container image"));
            return;
        }
        let name = {
            let n = self.headless_name_entry.text().to_string();
            if n.is_empty() {
                format!("headless-{}", chrono::Local::now().timestamp())
            } else {
                n
            }
        };
        let cmd = {
            let c = self.headless_cmd_entry.text().to_string();
            if c.is_empty() {
                None
            } else {
                Some(c)
            }
        };
        let args = {
            let v: Vec<String> = self
                .headless_args_entry
                .text()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        };
        let replicas = self.headless_replicas.value() as u32;
        // Flexible (switch off) → send cores/ram/gpus = 0 so the platform
        // allocates; Fixed (switch on) → use the resource selector. Mirrors
        // LaunchHeadlessAsync (HeadlessResourceType == "fixed" ? selected : 0).
        let fixed = self.headless_resource_type_switch.is_active();
        let sel_cores = self.headless_resource_selector.cores();
        let sel_ram = self.headless_resource_selector.ram();
        let sel_gpus = self.headless_resource_selector.gpus();
        let (cores, ram, gpus) = if fixed {
            (sel_cores, sel_ram, sel_gpus)
        } else {
            (0, 0, 0)
        };

        let params = SessionLaunchParams {
            name: name.clone(),
            image,
            session_type: "headless".to_string(),
            cores,
            ram,
            gpus,
            cmd,
            env: None,
            registry_username: None,
            registry_secret: None,
            args,
            replicas: Some(replicas),
        };

        self.headless_launch_btn.set_sensitive(false);
        self.status_label
            .set_text(crate::tr_en!("Launching batch job…"));

        let svc = self.services.clone();
        let params_clone = params.clone();
        let result = self
            .services
            .spawn(async move {
                let Some(token) = svc.get_token().await else {
                    return Err("Not authenticated".to_string());
                };
                // One POST per replica: asking for eight jobs and receiving one
                // is what the single-request version did.
                svc.sessions
                    .launch_headless(&token, &params_clone)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

        match result {
            Ok(ids) => {
                let id = ids.join(", ");
                self.status_label.set_text("");
                self.services.toast.toast(if ids.len() > 1 {
                    crate::tr_fmt!("Launched {} batch replicas ({})", ids.len(), &id)
                } else {
                    crate::tr_fmt!("Launched batch job '{}' ({})", name, &id)
                });

                // Save to recent launches so the batch job can be relaunched with
                // its exact command line (cmd/args/replicas) and resources.
                let now = chrono::Local::now().to_rfc3339();
                let recent = RecentLaunch {
                    name: params.name.clone(),
                    session_type: "headless".to_string(),
                    image: params.image.clone(),
                    // Preserve the selected resources in the record even when
                    // flexible (they display only for Fixed); a flexible relaunch
                    // re-zeroes them via to_launch_params.
                    cores: sel_cores,
                    ram: sel_ram,
                    gpus: sel_gpus,
                    timestamp: now.clone(),
                    // The headless tab exposes no project selector.
                    project: None,
                    resource_type: Some(if fixed { "fixed" } else { "flexible" }.to_string()),
                    cmd: params.cmd.clone(),
                    args: params.args.clone(),
                    replicas: params.replicas,
                    launched_at: Some(now),
                };
                let _ = self.services.recent_launches.save(recent);

                self.show_result(
                    true,
                    &crate::tr_fmt!("{} submitted", params.name),
                    &crate::tr_fmt!("Batch job \u{00B7} {}", params.image),
                );
                glib::timeout_future(std::time::Duration::from_millis(RESULT_DWELL_MS as u64))
                    .await;
                if let Some(ref cb) = *self.on_launched.borrow() {
                    cb();
                }
            }
            Err(e) => {
                self.show_result(false, crate::tr_en!("The batch job was not submitted"), &e);
            }
        }
        self.headless_launch_btn.set_sensitive(true);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

#[cfg(test)]
mod result_tests {
    #[test]
    fn a_launch_answers_inside_the_modal_rather_than_over_it() {
        // The form lives in a modal, so a result dialog raised from here is a
        // modal on a modal — two things to dismiss for one action, and the
        // arrangement that once left the app frozen behind a window it had
        // opened itself. The result goes where the form was.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("launch_form.rs")));
        assert!(
            !code.contains("show_launch_dialog"),
            "the launch form opens a dialog on top of the modal it lives in"
        );
        assert!(
            code.contains("self.show_result("),
            "the launch form no longer shows its result in place"
        );
    }

    #[test]
    fn a_failure_leaves_a_way_back_and_a_success_does_not_need_one() {
        // A success closes the modal on its own, so a button there is one
        // nobody can press in time. A failure stays put and has to be
        // recoverable without closing and re-opening the whole form.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("launch_form.rs")));
        let at = code.find("fn show_result").expect("show_result is gone");
        let body = &code[at..(at + 1400).min(code.len())];
        assert!(
            body.contains("result_back_btn.set_visible(!ok)"),
            "the way back is not tied to the outcome"
        );
    }
}


#[cfg(test)]
mod use_this_image_tests {
    use super::*;

    const SOURCE: &str = include_str!("launch_form.rs");

    fn image(id: &str, types: &[&str]) -> ParsedImage {
        ParsedImage {
            id: id.to_string(),
            registry: "images.canfar.net".into(),
            project: "skaha".into(),
            version: "1.0".into(),
            types: types.iter().map(|t| t.to_string()).collect(),
            display_name: "astroml:1.0".into(),
        }
    }

    #[test]
    fn an_image_the_standard_tab_launches_is_reachable_there() {
        let img = image("images.canfar.net/skaha/astroml:1.0", &["notebook"]);
        assert_eq!(reachable_type(&img).map(String::as_str), Some("notebook"));
    }

    #[test]
    fn a_headless_only_image_is_not_reachable_in_the_standard_tab() {
        // Skaha advertises types the Standard tab does not launch. Selecting
        // the dropdown index for one anyway would land on whatever image
        // happened to sit at that position — a different image entirely,
        // silently.
        let img = image("images.canfar.net/skaha/probe:1.0", &["headless"]);
        assert_eq!(reachable_type(&img), None);
    }

    #[test]
    fn an_image_with_no_types_at_all_is_not_reachable() {
        // Discovery knows about images Skaha never listed a type for.
        assert_eq!(reachable_type(&image("private/thing:1", &[])), None);
    }

    #[test]
    fn the_first_launchable_type_wins() {
        let img = image("x:1", &["headless", "notebook", "carta"]);
        assert_eq!(reachable_type(&img).map(String::as_str), Some("notebook"));
    }

    #[test]
    fn an_unreachable_image_falls_back_rather_than_being_dropped() {
        // The reference does this too: an image the catalogue cannot offer for
        // a session type goes to the custom-image field, because the user asked
        // for it and "nothing happened" is the worst possible answer.
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("pub fn select_image_by_id")
            .expect("select_image_by_id is gone");
        let end = code[at..]
            .find("\n    }\n")
            .map(|e| at + e)
            .unwrap_or(code.len());
        assert!(
            code[at..end].contains("self.apply_picked_image(image_id)"),
            "an image the Standard tab cannot show is dropped on the floor"
        );
    }

    #[test]
    fn the_list_shown_and_the_list_launched_are_the_same_list() {
        // `update_images` fills the dropdown and `get_selected_image_id` reads
        // it back; each used to spell out the type/registry/project filter for
        // itself, and selecting by id needed it a third time. Two copies of a
        // filter is two chances for the row you picked and the row that
        // launches to disagree — silently, since both produce a valid image.
        //
        // (The projects scan is deliberately not included: it filters over
        // CANDIDATE projects rather than the selected one, which is a different
        // question.)
        let code = crate::testing::code(SOURCE);
        for caller in ["fn update_images", "fn get_selected_image_id"] {
            let at = code
                .find(caller)
                .unwrap_or_else(|| panic!("{caller} is gone"));
            let end = code[at..]
                .find("\n    }\n")
                .map(|e| at + e)
                .unwrap_or(code.len());
            let body = &code[at..end];
            assert!(
                body.contains("self.visible_images()"),
                "{caller} decides for itself which images are showing"
            );
            assert!(
                !body.contains("images_for_type_registry_and_project("),
                "{caller} still has its own copy of the filter"
            );
        }
    }
}
