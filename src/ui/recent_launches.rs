use crate::models::RecentLaunch;
use crate::state::AppServices;
use crate::ui::session_icon::session_type_icon;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct RecentLaunchesView {
    pub container: gtk::Box,
    list_box: gtk::ListBox,
    filter_entry: gtk::SearchEntry,
    services: Arc<AppServices>,
    #[allow(clippy::type_complexity)]
    on_relaunch: Rc<RefCell<Option<Box<dyn Fn(RecentLaunch)>>>>,
    session_limit_reached: Rc<RefCell<bool>>,
}

impl RecentLaunchesView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.add_css_class("card");
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(12);
        container.set_margin_bottom(12);

        // Header
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(12);

        let title = gtk::Label::new(Some(crate::tr_en!("Recent Launches")));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        let clear_btn = gtk::Button::from_icon_name("edit-clear-all-symbolic");
        clear_btn.set_tooltip_text(Some(crate::tr_en!("Clear history")));
        header.append(&clear_btn);
        container.append(&header);

        // Filter
        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_placeholder_text(Some(crate::tr_en!("Filter...")));
        filter_entry.set_margin_start(12);
        filter_entry.set_margin_end(12);
        container.append(&filter_entry);

        // List
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_min_content_height(150);
        scrolled.set_vexpand(true);

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_bottom(12);
        scrolled.set_child(Some(&list_box));
        container.append(&scrolled);

        let view = Rc::new(RecentLaunchesView {
            container,
            list_box,
            filter_entry,
            services,
            on_relaunch: Rc::new(RefCell::new(None)),
            session_limit_reached: Rc::new(RefCell::new(false)),
        });

        // Clear button
        {
            let view = view.clone();
            clear_btn.connect_clicked(move |_| {
                let _ = view.services.recent_launches.clear();
                view.refresh();
            });
        }

        // Filter
        {
            let view_clone = view.clone();
            let filter_entry = view.filter_entry.clone();
            filter_entry.connect_search_changed(move |_| {
                view_clone.refresh();
            });
        }

        view
    }

    pub fn set_on_relaunch(&self, callback: impl Fn(RecentLaunch) + 'static) {
        *self.on_relaunch.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_session_limit_reached(&self, reached: bool) {
        *self.session_limit_reached.borrow_mut() = reached;
        self.refresh();
    }

    pub fn refresh(&self) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let launches = self.services.recent_launches.load();
        let filter = self.filter_entry.text().to_string().to_lowercase();

        let limit_reached = *self.session_limit_reached.borrow();
        let now = chrono::Local::now().to_rfc3339();

        for (idx, launch) in launches.iter().enumerate() {
            if !filter.is_empty() {
                let matches = launch.name.to_lowercase().contains(&filter)
                    || launch.session_type.to_lowercase().contains(&filter)
                    || launch.image.to_lowercase().contains(&filter);
                if !matches {
                    continue;
                }
            }

            // Project / image (project omitted for custom + headless entries).
            let img_part = match launch.project_display() {
                Some(project) => format!("{}/{}", project, launch.display_image()),
                None => launch.display_image(),
            };
            // Flexible vs Fixed (with the exact resources when fixed).
            let resource_part = if launch.is_flexible() {
                launch.resource_type_display().to_string()
            } else {
                let mut s = format!(
                    "{} · CPU:{} RAM:{}G",
                    launch.resource_type_display(),
                    launch.cores,
                    launch.ram,
                );
                if launch.gpus > 0 {
                    s.push_str(&format!(" GPU:{}", launch.gpus));
                }
                s
            };

            let row = adw::ActionRow::builder()
                .title(&launch.name)
                .subtitle(format!(
                    "{} | {} | {} | {}",
                    launch.type_display(),
                    img_part,
                    resource_part,
                    launch.relative_date(&now),
                ))
                .build();

            let icon = session_type_icon(&launch.session_type, 32);
            row.add_prefix(&icon);

            let relaunch_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
            relaunch_btn.set_tooltip_text(Some(crate::tr_en!("Relaunch")));
            relaunch_btn.set_valign(gtk::Align::Center);
            relaunch_btn.set_sensitive(!limit_reached);
            {
                let launch = launch.clone();
                let on_relaunch = self.on_relaunch.clone();
                relaunch_btn.connect_clicked(move |_| {
                    if let Some(ref cb) = *on_relaunch.borrow() {
                        cb(launch.clone());
                    }
                });
            }
            row.add_suffix(&relaunch_btn);

            let remove_btn = gtk::Button::from_icon_name("edit-delete-symbolic");
            remove_btn.set_tooltip_text(Some(crate::tr_en!("Remove")));
            remove_btn.set_valign(gtk::Align::Center);
            {
                let services = self.services.clone();
                let list_box = self.list_box.clone();
                remove_btn.connect_clicked(move |_| {
                    let _ = services.recent_launches.remove(idx);
                    // Simple refresh by removing the row
                    if let Some(row) = list_box.row_at_index(idx as i32) {
                        list_box.remove(&row);
                    }
                });
            }
            row.add_suffix(&remove_btn);

            self.list_box.append(&row);
        }

        if launches.is_empty() || (self.list_box.first_child().is_none()) {
            let empty = gtk::Label::new(Some(crate::tr_en!("No recent launches")));
            empty.add_css_class("dim-label");
            empty.set_margin_top(12);
            empty.set_margin_bottom(12);
            self.list_box.append(&empty);
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
