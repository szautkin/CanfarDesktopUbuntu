use crate::helpers::fits_renderer::{self, ColorMap, Stretch};
use crate::models::FitsImageData;
use crate::ui::fits_canvas::FitsCanvas;
use crate::ui::fits_controls::FitsControls;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub struct FitsTab {
    widget: gtk::Box,
    canvas: Rc<FitsCanvas>,
    controls: Rc<FitsControls>,
    data: Rc<FitsImageData>,
}

impl FitsTab {
    pub fn new(data: FitsImageData, shared_cursor: Rc<RefCell<Option<(f64, f64)>>>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        let data = Rc::new(data);

        // Initial render
        let rgba = fits_renderer::render_to_rgba(
            &data,
            Stretch::Linear,
            ColorMap::Grayscale,
            data.min_val,
            data.max_val,
        );

        let canvas = FitsCanvas::new(
            data.width,
            data.height,
            rgba,
            shared_cursor.clone(),
            data.wcs.clone(),
        );

        let controls = FitsControls::new(data.min_val, data.max_val);
        widget.append(controls.widget());
        widget.append(canvas.widget());

        let tab = Rc::new(FitsTab {
            widget,
            canvas,
            controls,
            data,
        });

        // Wire controls to re-render
        let t = tab.clone();
        tab.controls.set_on_changed(move || {
            t.re_render();
        });

        tab
    }

    fn re_render(&self) {
        let stretch = self.controls.stretch();
        let colormap = self.controls.colormap();
        let (vmin, vmax) = self.controls.range();

        let rgba = fits_renderer::render_to_rgba(&self.data, stretch, colormap, vmin, vmax);
        self.canvas.update_image(rgba);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}
