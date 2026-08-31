//! The Save-as-PNG-or-PDF dialog, for any viewer that can compose a figure.
//!
//! Preview on the left, output controls on the right, a file picker behind
//! Save. All of that is about formats and files; none of it is about cubes or
//! images. The cube viewer had it inline, and the FITS viewer wanting the same
//! dialog is exactly the moment to take it out rather than write a second one
//! that drifts — one ending up with a 2× default and the other 1×, one adding
//! the extension you forgot and the other not.
//!
//! What a caller supplies is a [`Compose`]: "give me the figure at this scale,
//! with or without a transparent ground". Everything else lives here.

use crate::helpers::pdf_writer;
use gtk4::cairo::ImageSurface;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Scale multipliers the picker offers.
///
/// Capped at 4 deliberately: `export_cube_figure` and `export_fits_figure`
/// clamp their `scale` argument to 1..4, and a picker offering 8× would mean
/// the same export succeeded from the UI and was silently clamped for an agent.
pub const EXPORT_SCALES: [i32; 3] = [1, 2, 4];

/// Index into [`EXPORT_SCALES`], not a factor. A bare `1` here once meant "the
/// second entry" while a bare `2` elsewhere meant "2×"; they agreed by luck.
pub const DEFAULT_EXPORT_SCALE: usize = 1;

pub const EXPORT_FORMATS: [&str; 2] = ["PNG", "PDF"];

/// Compose the figure at `scale`, with a transparent ground or without.
pub type Compose = Rc<dyn Fn(i32, bool) -> Option<ImageSurface>>;

/// A file name without a directory or an extension, safe to suggest.
pub fn base_name(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "figure".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Show the export dialog.
///
/// `title` names the figure and seeds the suggested file name; `compose` is
/// asked for the pixels each time Save is pressed, so the scale and background
/// the user chose are the ones rendered rather than a stale preview.
pub fn show(parent: &impl IsA<gtk::Widget>, title: &str, compose: Compose) {
    let parent_widget: gtk::Widget = parent.clone().upcast::<gtk::Widget>();
    let title = title.to_string();

    // The shared shell: content scrolls, actions stay pinned. The cube's copy
    // of this dialog built its own window and sat on the "not yet migrated"
    // list; moving it here takes it off that list rather than adding a second
    // entry to it.
    let dialog =
        crate::ui::dialog::Dialog::new(crate::tr_en!("Export Figure"), crate::ui::fit::BROWSE, 620);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    body.set_hexpand(true);
    body.set_vexpand(true);

    // ── Left: what you are about to save ────────────────────────────────────
    let surf_cell: Rc<RefCell<Option<ImageSurface>>> = Rc::new(RefCell::new(None));
    let preview = gtk::DrawingArea::new();
    preview.set_hexpand(true);
    preview.set_vexpand(true);
    preview.set_content_width(520);
    preview.set_content_height(560);
    {
        let surf_cell = surf_cell.clone();
        preview.set_draw_func(move |_area, cr, w, h| {
            cr.set_source_rgb(0.12, 0.12, 0.13);
            let _ = cr.paint();
            let Some(surf) = surf_cell.borrow().clone() else {
                return;
            };
            let (sw, sh) = (surf.width() as f64, surf.height() as f64);
            if sw <= 0.0 || sh <= 0.0 {
                return;
            }
            // Fit, never enlarge: a preview blown up past its own resolution
            // shows a softness the exported file will not have.
            let scale = (w as f64 / sw).min(h as f64 / sh).min(1.0);
            let (dw, dh) = (sw * scale, sh * scale);
            let _ = cr.save();
            cr.translate((w as f64 - dw) / 2.0, (h as f64 - dh) / 2.0);
            cr.scale(scale, scale);
            let _ = cr.set_source_surface(&surf, 0.0, 0.0);
            let _ = cr.paint();
            let _ = cr.restore();
        });
    }
    let preview_frame = gtk::Frame::new(None);
    preview_frame.set_child(Some(&preview));
    preview_frame.set_hexpand(true);
    preview_frame.set_vexpand(true);
    body.append(&preview_frame);

    // ── Right: what to save ─────────────────────────────────────────────────
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 12);
    controls.set_width_request(260);

    let group = adw::PreferencesGroup::new();

    let scale_row = adw::ComboRow::new();
    scale_row.set_title(crate::tr_en!("Scale"));
    let labels: Vec<String> = EXPORT_SCALES
        .iter()
        .map(|f| format!("{f}\u{00D7}"))
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    scale_row.set_model(Some(&gtk::StringList::new(&label_refs)));
    scale_row.set_selected(DEFAULT_EXPORT_SCALE as u32);
    group.add(&scale_row);

    let transparent_row = adw::SwitchRow::new();
    transparent_row.set_title(crate::tr_en!("Transparent background"));
    transparent_row.set_active(false);
    group.add(&transparent_row);

    let format_row = adw::ComboRow::new();
    format_row.set_title(crate::tr_en!("Format"));
    format_row.set_model(Some(&gtk::StringList::new(&EXPORT_FORMATS)));
    format_row.set_selected(0);
    group.add(&format_row);

    controls.append(&group);

    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.add_css_class("dim-label");

    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let save_btn = gtk::Button::with_label(crate::tr_en!("Save"));
    save_btn.add_css_class("suggested-action");

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    controls.append(&spacer);
    controls.append(&status);
    body.append(&controls);

    dialog.content().append(&body);
    dialog.add_secondary_action(&cancel_btn);
    dialog.add_action(&save_btn);
    let window = dialog.window.clone();

    // The preview follows the controls, so what is shown is what Save writes.
    let refresh = {
        let compose = compose.clone();
        let surf_cell = surf_cell.clone();
        let preview = preview.clone();
        let scale_row = scale_row.clone();
        let transparent_row = transparent_row.clone();
        Rc::new(move || {
            let scale = EXPORT_SCALES
                .get(scale_row.selected() as usize)
                .copied()
                .unwrap_or(EXPORT_SCALES[DEFAULT_EXPORT_SCALE]);
            *surf_cell.borrow_mut() = compose(scale, transparent_row.is_active());
            preview.queue_draw();
        })
    };
    refresh();
    {
        let refresh = refresh.clone();
        scale_row.connect_selected_notify(move |_| refresh());
    }
    {
        let refresh = refresh.clone();
        transparent_row.connect_active_notify(move |_| refresh());
    }

    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }
    {
        let window = window.clone();
        save_btn.connect_clicked(move |_| {
            let scale = EXPORT_SCALES
                .get(scale_row.selected() as usize)
                .copied()
                .unwrap_or(EXPORT_SCALES[DEFAULT_EXPORT_SCALE]);
            let is_pdf = EXPORT_FORMATS.get(format_row.selected() as usize) == Some(&"PDF");

            status.set_text(crate::tr_en!("Rendering figure…"));
            let Some(mut surface) = compose(scale, transparent_row.is_active()) else {
                status.set_text(crate::tr_en!("Could not compose the figure."));
                return;
            };
            let (w, h, rgba) = crate::helpers::image_bytes::surface_to_rgba(&mut surface);

            let (status, parent_widget, window, title) = (
                status.clone(),
                parent_widget.clone(),
                window.clone(),
                title.clone(),
            );
            glib::spawn_future_local(async move {
                let ext = if is_pdf { "pdf" } else { "png" };
                let filter = gtk::FileFilter::new();
                if is_pdf {
                    filter.set_name(Some(crate::tr_en!("PDF Document")));
                    filter.add_pattern("*.pdf");
                } else {
                    filter.set_name(Some(crate::tr_en!("PNG Image")));
                    filter.add_pattern("*.png");
                }
                let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);

                let chooser = gtk::FileDialog::builder()
                    .title(crate::tr_en!("Export Figure"))
                    .modal(true)
                    .initial_name(format!("{}.{}", base_name(&title), ext))
                    .filters(&filters)
                    .build();

                let root = window.clone().upcast::<gtk::Window>();
                let Ok(file) = chooser.save_future(Some(&root)).await else {
                    status.set_text("");
                    return;
                };
                let Some(mut path) = file.path() else {
                    return;
                };
                // Someone who types "m51" means "m51.png".
                if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref()
                    != Some(ext)
                {
                    path.set_extension(ext);
                }

                let result = if is_pdf {
                    pdf_writer::write_pdf(&path, w, h, &rgba)
                } else {
                    pdf_writer::write_png(&path, w, h, &rgba)
                };
                match result {
                    Ok(()) => {
                        crate::ui::toast::show(
                            &parent_widget,
                            &crate::tr_fmt!("Saved {}", path.display()),
                        );
                        status.set_text("");
                        window.close();
                    }
                    Err(e) => status.set_text(&crate::tr_fmt!("Export failed: {}", e)),
                }
            });
        });
    }

    dialog.present(parent);
}

#[cfg(test)]
mod tests {
    use super::{base_name, DEFAULT_EXPORT_SCALE, EXPORT_SCALES};

    /// The default is an INDEX, and it points at a real entry.
    #[test]
    fn the_default_scale_is_a_real_choice() {
        assert!(DEFAULT_EXPORT_SCALE < EXPORT_SCALES.len());
        assert_eq!(EXPORT_SCALES[DEFAULT_EXPORT_SCALE], 2, "2x by default");
    }

    /// A factor of 0 or a negative would compose an empty or inverted surface.
    #[test]
    fn every_scale_actually_enlarges() {
        for factor in EXPORT_SCALES {
            assert!(factor >= 1, "{factor}x is not a usable export scale");
        }
    }

    /// The picker never offers a scale the export tools would clamp.
    ///
    /// Both `export_cube_figure` and `export_fits_figure` cap `scale` at 4. A
    /// picker offering 8× would mean the same export succeeded from the UI and
    /// was silently reduced for an agent.
    #[test]
    fn the_ui_never_offers_a_scale_a_tool_would_reject() {
        let ui_max = EXPORT_SCALES.iter().copied().max().unwrap_or(1);
        assert!(
            ui_max <= 4,
            "the picker offers {ui_max}x against a cap of 4"
        );
    }

    /// A suggested file name is always usable.
    #[test]
    fn a_suggested_name_is_never_empty_or_pathlike() {
        assert_eq!(base_name("M51 core"), "M51_core");
        assert_eq!(base_name("  "), "figure");
        assert_eq!(base_name("///"), "figure");
        // A title that could walk out of the chosen directory cannot.
        assert!(!base_name("../../etc/passwd").contains('/'));
        assert!(!base_name("../../etc/passwd").contains(".."));
    }
}
