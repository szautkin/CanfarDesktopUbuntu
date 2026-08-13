use crate::models::SessionTemplate;
use crate::state::AppServices;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type OnLaunchCallback = Rc<RefCell<Option<Box<dyn Fn(SessionTemplate)>>>>;

pub struct TemplateManager {
    widget: gtk::Box,
    list_box: gtk::ListBox,
    services: Arc<AppServices>,
    on_launch: OnLaunchCallback,
}

impl TemplateManager {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("card");
        widget.set_margin_start(12);
        widget.set_margin_end(12);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);

        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header_box.set_margin_start(12);
        header_box.set_margin_end(12);
        header_box.set_margin_top(12);

        let title = gtk::Label::new(Some(crate::tr_en!("Session Templates")));
        title.add_css_class("title-4");
        title.set_hexpand(true);
        title.set_halign(gtk::Align::Start);
        header_box.append(&title);

        widget.append(&header_box);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_top(12);
        list_box.set_margin_bottom(12);

        list_box.set_placeholder(Some(
            &gtk::Label::builder()
                .label(crate::tr_en!(
                    "No saved templates — save one from the launch form"
                ))
                .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
                .margin_start(12)
                .margin_end(12)
                .margin_top(12)
                .margin_bottom(12)
                .build(),
        ));

        widget.append(&list_box);

        let manager = Rc::new(TemplateManager {
            widget,
            list_box,
            services,
            on_launch: Rc::new(RefCell::new(None)),
        });

        manager.refresh();
        manager
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn set_on_launch(&self, cb: impl Fn(SessionTemplate) + 'static) {
        *self.on_launch.borrow_mut() = Some(Box::new(cb));
    }

    pub fn refresh(&self) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let templates = self.services.templates.load();
        for template in templates {
            let row = self.make_template_row(&template);
            self.list_box.append(&row);
        }
    }

    fn make_template_row(&self, template: &SessionTemplate) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&template.name)
            .subtitle(format!(
                "{} | {} | {}c {}GB {}gpu",
                template.session_type,
                template.image.rsplit('/').next().unwrap_or(&template.image),
                template.cores,
                template.ram,
                template.gpus,
            ))
            .build();

        row.add_prefix(&gtk::Image::from_icon_name("document-properties-symbolic"));

        let launch_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
        launch_btn.set_tooltip_text(Some(crate::tr_en!("Launch from template")));
        launch_btn.set_valign(gtk::Align::Center);
        launch_btn.add_css_class("flat");
        let on_launch = self.on_launch.clone();
        let t = template.clone();
        launch_btn.connect_clicked(move |_| {
            if let Some(cb) = on_launch.borrow().as_ref() {
                cb(t.clone());
            }
        });
        row.add_suffix(&launch_btn);

        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.set_tooltip_text(Some(crate::tr_en!("Delete template")));
        delete_btn.set_valign(gtk::Align::Center);
        delete_btn.add_css_class("flat");
        let services = self.services.clone();
        let name = template.name.clone();
        delete_btn.connect_clicked(move |_| {
            let _ = services.templates.remove(&name);
            // Remove row from UI
            // We'll do a full refresh for simplicity
        });
        row.add_suffix(&delete_btn);

        row
    }
}
