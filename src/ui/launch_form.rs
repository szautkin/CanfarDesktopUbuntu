use crate::helpers::ImageParser;
use crate::models::{ParsedImage, RecentLaunch, Session, SessionLaunchParams};
use crate::state::AppServices;
use crate::ui::launch_dialog::show_launch_dialog;
use crate::ui::resource_selector::ResourceSelector;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

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
    status_label: gtk::Label,
    #[allow(clippy::type_complexity)]
    on_launched: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    session_limit_reached: Rc<RefCell<bool>>,
    active_sessions: Rc<RefCell<Vec<Session>>>,
    // Advanced tab
    custom_image_entry: adw::EntryRow,
    custom_type_combo: gtk::DropDown,
    adv_registry_combo: gtk::DropDown,
    registry_user_entry: adw::EntryRow,
    registry_secret_entry: adw::PasswordEntryRow,
    notebook: gtk::Notebook,
}

impl LaunchFormView {
    pub fn new(services: Arc<AppServices>, active_sessions: Rc<RefCell<Vec<Session>>>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_bottom(8);

        // Header
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(16);
        header.set_margin_end(16);
        header.set_margin_top(12);

        let title = gtk::Label::new(Some("Launch Session"));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        container.append(&header);

        // Tabs: Standard / Advanced
        let notebook = gtk::Notebook::new();
        notebook.set_margin_start(12);
        notebook.set_margin_end(12);
        notebook.set_margin_bottom(12);

        // === Standard Tab ===
        let standard_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        standard_box.set_margin_start(8);
        standard_box.set_margin_end(8);
        standard_box.set_margin_top(8);
        standard_box.set_margin_bottom(8);

        let form_group = adw::PreferencesGroup::new();

        // Session type
        let types_list =
            gtk::StringList::new(&["notebook", "desktop", "carta", "contributed", "firefly"]);
        let type_combo = gtk::DropDown::new(Some(types_list), gtk::Expression::NONE);
        let type_row = adw::ActionRow::builder().title("Session Type").build();
        type_row.add_suffix(&type_combo);
        form_group.add(&type_row);

        // Image Registry
        let registry_list = gtk::StringList::new(&[]);
        let registry_combo = gtk::DropDown::new(Some(registry_list), gtk::Expression::NONE);
        let registry_row = adw::ActionRow::builder().title("Image Registry").build();
        registry_row.add_suffix(&registry_combo);
        form_group.add(&registry_row);

        // Project
        let project_list = gtk::StringList::new(&[]);
        let project_combo = gtk::DropDown::new(Some(project_list), gtk::Expression::NONE);
        let project_row = adw::ActionRow::builder().title("Project").build();
        project_row.add_suffix(&project_combo);
        form_group.add(&project_row);

        // Image
        let image_list = gtk::StringList::new(&[]);
        let image_combo = gtk::DropDown::new(Some(image_list), gtk::Expression::NONE);
        let image_row = adw::ActionRow::builder().title("Container Image").build();
        image_row.add_suffix(&image_combo);
        form_group.add(&image_row);

        // Session name
        let name_entry = adw::EntryRow::builder().title("Session Name").build();
        form_group.add(&name_entry);

        standard_box.append(&form_group);

        // Resource type toggle
        let resource_group = adw::PreferencesGroup::new();
        let resource_type_switch = gtk::Switch::new();
        resource_type_switch.set_active(false);
        resource_type_switch.set_valign(gtk::Align::Center);

        let resource_row = adw::ActionRow::builder()
            .title("Fixed Resources")
            .subtitle("Enable to specify exact CPU/RAM/GPU")
            .build();
        resource_row.add_suffix(&resource_type_switch);
        resource_row.set_activatable_widget(Some(&resource_type_switch));
        resource_group.add(&resource_row);
        standard_box.append(&resource_group);

        // Resource selector
        let resource_selector = ResourceSelector::new();
        resource_selector.widget().set_visible(false);
        standard_box.append(resource_selector.widget());

        notebook.append_page(&standard_box, Some(&gtk::Label::new(Some("Standard"))));

        // === Advanced Tab ===
        let advanced_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        advanced_box.set_margin_start(8);
        advanced_box.set_margin_end(8);
        advanced_box.set_margin_top(8);
        advanced_box.set_margin_bottom(8);

        let adv_group = adw::PreferencesGroup::builder()
            .title("Custom Container Image")
            .description("Launch a session using a custom image URI")
            .build();

        // Session type
        let custom_type_list = gtk::StringList::new(&[
            "notebook",
            "desktop",
            "carta",
            "contributed",
            "firefly",
            "headless",
        ]);
        let custom_type_combo = gtk::DropDown::new(Some(custom_type_list), gtk::Expression::NONE);
        let custom_type_row = adw::ActionRow::builder().title("Session Type").build();
        custom_type_row.add_suffix(&custom_type_combo);
        adv_group.add(&custom_type_row);

        // Image Registry (from API repositories)
        let adv_registry_list = gtk::StringList::new(&[]);
        let adv_registry_combo = gtk::DropDown::new(Some(adv_registry_list), gtk::Expression::NONE);
        let adv_registry_row = adw::ActionRow::builder().title("Image Registry").build();
        adv_registry_row.add_suffix(&adv_registry_combo);
        adv_group.add(&adv_registry_row);

        // Custom image URI (project/name:tag)
        let custom_image_entry = adw::EntryRow::builder()
            .title("Image (project/name:tag)")
            .build();
        adv_group.add(&custom_image_entry);

        // Registry auth
        let auth_group = adw::PreferencesGroup::builder()
            .title("Registry Authentication")
            .description("Credentials for private registries. Leave blank for public images.")
            .build();

        let registry_user_entry = adw::EntryRow::builder().title("Username").build();
        auth_group.add(&registry_user_entry);

        let registry_secret_entry = adw::PasswordEntryRow::builder()
            .title("Token or Password")
            .build();
        auth_group.add(&registry_secret_entry);

        advanced_box.append(&adv_group);
        advanced_box.append(&auth_group);
        notebook.append_page(&advanced_box, Some(&gtk::Label::new(Some("Advanced"))));

        container.append(&notebook);

        // Status + Launch button
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom.set_margin_start(16);
        bottom.set_margin_end(16);
        bottom.set_margin_bottom(12);

        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_hexpand(true);
        status_label.set_halign(gtk::Align::Start);
        bottom.append(&status_label);

        let launch_btn = gtk::Button::with_label("Launch");
        launch_btn.add_css_class("suggested-action");
        bottom.append(&launch_btn);

        container.append(&bottom);

        // Toggle resource selector visibility
        {
            let resource_widget = resource_selector.widget().clone();
            resource_type_switch.connect_active_notify(move |switch| {
                resource_widget.set_visible(switch.is_active());
            });
        }

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
            status_label,
            on_launched: Rc::new(RefCell::new(None)),
            session_limit_reached: Rc::new(RefCell::new(false)),
            active_sessions,
            custom_image_entry,
            custom_type_combo,
            adv_registry_combo,
            registry_user_entry,
            registry_secret_entry,
            notebook,
        });

        // Type change -> update registries
        {
            let view_clone = view.clone();
            let type_combo = view.type_combo.clone();
            type_combo.connect_selected_notify(move |_| {
                view_clone.update_registries();
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

        view
    }

    pub fn set_on_launched(&self, callback: impl Fn() + 'static) {
        *self.on_launched.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_session_limit_reached(&self, reached: bool) {
        *self.session_limit_reached.borrow_mut() = reached;
        if reached {
            self.launch_btn.set_sensitive(false);
            self.status_label
                .set_text("Session limit reached (max 3 concurrent sessions)");
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
                let images = svc.images.get_images(&token).await?;
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
            Ok((raw_images, context, repos)) => {
                let parsed = ImageParser::parse_all(&raw_images);
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
                }
            }
            Err(e) => {
                self.status_label
                    .set_text(&format!("Failed to load images: {}", e));
            }
        }
    }

    fn selected_type(&self) -> String {
        let types = ["notebook", "desktop", "carta", "contributed", "firefly"];
        let idx = self.type_combo.selected() as usize;
        types.get(idx).unwrap_or(&"notebook").to_string()
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
        let registry = self.selected_registry();
        let images = self.images.borrow();

        let project = self.combo_selected_string(&self.project_combo);

        let filtered = ImageParser::images_for_type_registry_and_project(
            &images,
            &session_type,
            &registry,
            &project,
        );

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
        let name = format!("{}{}", session_type, count + 1);
        self.name_entry.set_text(&name);
    }

    fn get_selected_image_id(&self) -> Option<String> {
        let images = self.images.borrow();
        let session_type = self.selected_type();
        let registry = self.selected_registry();
        let project = self.combo_selected_string(&self.project_combo);

        let filtered = ImageParser::images_for_type_registry_and_project(
            &images,
            &session_type,
            &registry,
            &project,
        );
        let idx = self.image_combo.selected() as usize;
        filtered.get(idx).map(|img| img.id.clone())
    }

    fn update_advanced_name(&self) {
        let types = [
            "notebook",
            "desktop",
            "carta",
            "contributed",
            "firefly",
            "headless",
        ];
        let idx = self.custom_type_combo.selected() as usize;
        let session_type = types.get(idx).unwrap_or(&"notebook");
        let count = self.session_count_for_type(session_type);
        let name = format!("{}{}", session_type, count + 1);
        self.name_entry.set_text(&name);
    }

    async fn do_launch(&self) {
        if *self.session_limit_reached.borrow() {
            return;
        }

        let is_advanced = self.notebook.current_page() == Some(1);

        let (session_type, image, reg_user, reg_secret) = if is_advanced {
            let types = [
                "notebook",
                "desktop",
                "carta",
                "contributed",
                "firefly",
                "headless",
            ];
            let idx = self.custom_type_combo.selected() as usize;
            let st = types.get(idx).unwrap_or(&"notebook").to_string();
            // Prepend registry host to custom image path
            let registry_host = self.combo_selected_string(&self.adv_registry_combo);
            let custom_path = self.custom_image_entry.text().to_string();
            let img = if registry_host.is_empty() {
                custom_path
            } else {
                format!("{}/{}", registry_host, custom_path)
            };
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
                    self.status_label.set_text("Please select an image");
                    return;
                }
            };
            (st, img, None, None)
        };

        if image.is_empty() {
            self.status_label
                .set_text("Please select or enter an image");
            return;
        }

        let name = self.name_entry.text().to_string();
        if name.is_empty() {
            self.status_label.set_text("Please enter a session name");
            return;
        }

        let (cores, ram, gpus) = if self.resource_type_switch.is_active() {
            (
                self.resource_selector.cores(),
                self.resource_selector.ram(),
                self.resource_selector.gpus(),
            )
        } else {
            (
                self.services.endpoints.config().default_cores,
                self.services.endpoints.config().default_ram,
                0,
            )
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
        };

        self.launch_btn.set_sensitive(false);
        self.status_label.set_text("Launching session...");

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

        show_launch_dialog(
            &self.container,
            &name,
            &image_display,
            &session_type,
            cores,
            ram,
            gpus,
            launch_result.clone(),
        )
        .await;

        match launch_result {
            Ok(_) => {
                self.status_label.set_text("");

                // Save to recent launches
                let recent = RecentLaunch {
                    name,
                    session_type,
                    image,
                    cores,
                    ram,
                    gpus,
                    timestamp: chrono::Local::now().to_rfc3339(),
                };
                let _ = self.services.recent_launches.save(recent);

                if let Some(ref cb) = *self.on_launched.borrow() {
                    cb();
                }
            }
            Err(e) => {
                self.status_label.set_text(&format!("Launch failed: {}", e));
            }
        }

        self.launch_btn.set_sensitive(true);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
