use crate::helpers::fits_renderer::{ColorMap, Stretch};
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub struct FitsControls {
    widget: gtk::Box,
    stretch_combo: gtk::DropDown,
    colormap_combo: gtk::DropDown,
    min_spin: gtk::SpinButton,
    max_spin: gtk::SpinButton,
    on_changed: Rc<RefCell<Option<Box<dyn Fn()>>>>,
}

impl FitsControls {
    pub fn new(min_val: f64, max_val: f64) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        widget.set_margin_start(8);
        widget.set_margin_end(8);
        widget.set_margin_top(4);
        widget.set_margin_bottom(4);

        // Stretch
        widget.append(&gtk::Label::new(Some("Stretch:")));
        let stretch_items = gtk::StringList::new(&["Linear", "Log", "Sqrt", "Histogram Eq"]);
        let stretch_combo = gtk::DropDown::new(Some(stretch_items), gtk::Expression::NONE);
        stretch_combo.set_selected(0);
        widget.append(&stretch_combo);

        // Color map
        widget.append(&gtk::Label::new(Some("Color:")));
        let cmap_items = gtk::StringList::new(&["Grayscale", "Heat", "Viridis"]);
        let colormap_combo = gtk::DropDown::new(Some(cmap_items), gtk::Expression::NONE);
        colormap_combo.set_selected(0);
        widget.append(&colormap_combo);

        // Min/Max range
        widget.append(&gtk::Label::new(Some("Min:")));
        let min_adj = gtk::Adjustment::new(min_val, min_val, max_val, 1.0, 10.0, 0.0);
        let min_spin = gtk::SpinButton::new(Some(&min_adj), 1.0, 1);
        min_spin.set_width_chars(8);
        widget.append(&min_spin);

        widget.append(&gtk::Label::new(Some("Max:")));
        let max_adj = gtk::Adjustment::new(max_val, min_val, max_val, 1.0, 10.0, 0.0);
        let max_spin = gtk::SpinButton::new(Some(&max_adj), 1.0, 1);
        max_spin.set_width_chars(8);
        widget.append(&max_spin);

        let on_changed: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));

        let controls = Rc::new(FitsControls {
            widget,
            stretch_combo,
            colormap_combo,
            min_spin,
            max_spin,
            on_changed,
        });

        // Connect signals
        let c = controls.clone();
        controls.stretch_combo.connect_selected_notify(move |_| {
            if let Some(cb) = c.on_changed.borrow().as_ref() {
                cb();
            }
        });

        let c = controls.clone();
        controls.colormap_combo.connect_selected_notify(move |_| {
            if let Some(cb) = c.on_changed.borrow().as_ref() {
                cb();
            }
        });

        let c = controls.clone();
        controls.min_spin.connect_value_changed(move |_| {
            if let Some(cb) = c.on_changed.borrow().as_ref() {
                cb();
            }
        });

        let c = controls.clone();
        controls.max_spin.connect_value_changed(move |_| {
            if let Some(cb) = c.on_changed.borrow().as_ref() {
                cb();
            }
        });

        controls
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn set_on_changed(&self, cb: impl Fn() + 'static) {
        *self.on_changed.borrow_mut() = Some(Box::new(cb));
    }

    pub fn stretch(&self) -> Stretch {
        match self.stretch_combo.selected() {
            1 => Stretch::Log,
            2 => Stretch::Sqrt,
            3 => Stretch::HistogramEq,
            _ => Stretch::Linear,
        }
    }

    pub fn colormap(&self) -> ColorMap {
        match self.colormap_combo.selected() {
            1 => ColorMap::Heat,
            2 => ColorMap::Viridis,
            _ => ColorMap::Grayscale,
        }
    }

    pub fn range(&self) -> (f64, f64) {
        (self.min_spin.value(), self.max_spin.value())
    }
}
