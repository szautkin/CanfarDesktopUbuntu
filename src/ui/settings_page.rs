use crate::config::AppConfig;
use crate::state::AppServices;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct SettingsPage {
    pub widget: adw::PreferencesPage,
    services: Arc<AppServices>,
    config: Rc<RefCell<AppConfig>>,
}

impl SettingsPage {
    pub fn new(services: Arc<AppServices>) -> Self {
        let config = Rc::new(RefCell::new(services.settings.load()));
        let widget = adw::PreferencesPage::new();
        widget.set_title("Settings");
        widget.set_icon_name(Some("emblem-system-symbolic"));

        let page = SettingsPage {
            widget,
            services,
            config,
        };
        page.build_appearance_group();
        page.build_defaults_group();
        page.build_connection_group();
        page.build_about_group();
        page
    }

    fn build_appearance_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title("Appearance");

        let theme_row = adw::ComboRow::new();
        theme_row.set_title("Theme");
        theme_row.set_subtitle("Choose how Verbinal looks");
        let themes = gtk::StringList::new(&["System", "Light", "Dark"]);
        theme_row.set_model(Some(&themes));

        // Set initial value
        let current = self.config.borrow().theme.clone();
        let idx = match current.as_str() {
            "Light" => 1,
            "Dark" => 2,
            _ => 0,
        };
        theme_row.set_selected(idx);

        let config = self.config.clone();
        let services = self.services.clone();
        theme_row.connect_selected_notify(move |row| {
            let theme = match row.selected() {
                1 => "Light",
                2 => "Dark",
                _ => "System",
            };
            config.borrow_mut().theme = theme.to_string();
            apply_theme(theme);
            let _ = services.settings.save(&config.borrow());
        });

        group.add(&theme_row);
        self.widget.add(&group);
    }

    fn build_defaults_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title("Session Defaults");
        group.set_description(Some("Default values for new session launches"));

        // Default session type
        let type_row = adw::ComboRow::new();
        type_row.set_title("Session Type");
        let types =
            gtk::StringList::new(&["notebook", "desktop", "carta", "contributed", "firefly"]);
        type_row.set_model(Some(&types));

        let current_type = self.config.borrow().default_session_type.clone();
        let type_names = ["notebook", "desktop", "carta", "contributed", "firefly"];
        let type_idx = type_names
            .iter()
            .position(|&t| t == current_type)
            .unwrap_or(0) as u32;
        type_row.set_selected(type_idx);

        let config = self.config.clone();
        let services = self.services.clone();
        type_row.connect_selected_notify(move |row| {
            let selected = row.selected() as usize;
            if selected < type_names.len() {
                config.borrow_mut().default_session_type = type_names[selected].to_string();
                let _ = services.settings.save(&config.borrow());
            }
        });
        group.add(&type_row);

        // Default CPU cores
        let cores_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                self.config.borrow().default_cores as f64,
                1.0,
                16.0,
                1.0,
                1.0,
                0.0,
            )),
            1.0,
            0,
        );
        cores_row.set_title("Default CPU Cores");
        let config = self.config.clone();
        let services = self.services.clone();
        cores_row.connect_value_notify(move |row| {
            config.borrow_mut().default_cores = row.value() as u32;
            let _ = services.settings.save(&config.borrow());
        });
        group.add(&cores_row);

        // Default RAM
        let ram_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                self.config.borrow().default_ram as f64,
                1.0,
                256.0,
                1.0,
                4.0,
                0.0,
            )),
            1.0,
            0,
        );
        ram_row.set_title("Default RAM (GB)");
        let config = self.config.clone();
        let services = self.services.clone();
        ram_row.connect_value_notify(move |row| {
            config.borrow_mut().default_ram = row.value() as u32;
            let _ = services.settings.save(&config.borrow());
        });
        group.add(&ram_row);

        self.widget.add(&group);
    }

    fn build_connection_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title("Connection");
        group.set_description(Some("CANFAR API endpoint configuration"));

        let url_row = adw::EntryRow::new();
        url_row.set_title("API Base URL");
        url_row.set_text(&self.config.borrow().api_base_url);

        let config = self.config.clone();
        let services = self.services.clone();
        url_row.connect_changed(move |row| {
            let text = row.text().to_string();
            if !text.is_empty() {
                config.borrow_mut().api_base_url = text;
                let _ = services.settings.save(&config.borrow());
            }
        });
        group.add(&url_row);

        // Reset button
        let reset_btn = gtk::Button::with_label("Reset to Defaults");
        reset_btn.add_css_class("destructive-action");
        reset_btn.set_halign(gtk::Align::Start);
        reset_btn.set_margin_top(8);

        let url_row_clone = url_row.clone();
        let config = self.config.clone();
        let services = self.services.clone();
        reset_btn.connect_clicked(move |_| {
            let defaults = AppConfig::default();
            url_row_clone.set_text(&defaults.api_base_url);
            *config.borrow_mut() = defaults;
            let _ = services.settings.save(&config.borrow());
        });
        group.add(&reset_btn);

        self.widget.add(&group);
    }

    fn build_about_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title("About");

        let version_row = adw::ActionRow::builder()
            .title("Version")
            .subtitle(env!("CARGO_PKG_VERSION"))
            .build();
        version_row.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
        group.add(&version_row);

        let platform_row = adw::ActionRow::builder()
            .title("Platform")
            .subtitle(format!(
                "{} / GTK4 + libadwaita / Rust",
                std::env::consts::OS
            ))
            .build();
        platform_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        group.add(&platform_row);

        self.widget.add(&group);
    }
}

pub fn apply_theme(theme: &str) {
    let manager = adw::StyleManager::default();
    let scheme = match theme {
        "Light" => adw::ColorScheme::ForceLight,
        "Dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::PreferLight,
    };
    manager.set_color_scheme(scheme);
}
