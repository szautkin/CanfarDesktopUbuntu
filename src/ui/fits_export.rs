//! Save what the FITS viewer is showing — or part of it — as PNG or PDF.
//!
//! The dialog is [`crate::ui::export_dialog`], shared with the cube viewer.
//! All this module does is turn a tab and a region into the [`Compose`] that
//! dialog asks for: pixels at a scale, with or without a ground.
//!
//! Its own file rather than more of `fits_viewer`, which is already the
//! largest in the tree.

use crate::ui::export_dialog::{self, Compose};
use crate::ui::fits_canvas::{DrawOpts, ViewRegion};
use crate::ui::fits_tab::FitsTab;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;

/// The size an unallocated view is exported at.
///
/// A tab that has never been on screen has no allocation — GTK gives one only
/// to a widget it has drawn — and a region measured against a zero-sized view
/// is not a region at all. This is the same fallback the agent captures use, so
/// an export and a `get_fits_image` of a hidden tab agree.
const FALLBACK_VIEW: (i32, i32) = (1024, 768);

/// Offer to save `region` of `tab`.
///
/// `region` is in the tab's own screen coordinates; `None` means the whole
/// view. Passing the region in rather than reading a stored one is what lets
/// the select-area gesture hand over what was just dragged without anything
/// having to remember it.
pub fn show(parent: &impl IsA<gtk::Widget>, tab: &Rc<FitsTab>, region: Option<ViewRegion>) {
    let canvas = tab.canvas().clone();
    let (view_w, view_h) = match canvas.view_size() {
        (w, h) if w > 0 && h > 0 => (w, h),
        _ => FALLBACK_VIEW,
    };
    let region = region
        .filter(ViewRegion::is_usable)
        .unwrap_or_else(|| ViewRegion::whole(view_w, view_h));

    let compose: Compose = Rc::new(move |scale, transparent| {
        // The output keeps the REGION's aspect, so a tall selection exports
        // tall. Deriving the height rather than taking it means the region is
        // never letterboxed into a frame of the wrong shape.
        let scale = scale.max(1);
        let out_w = (region.width.round() as i32).saturating_mul(scale).max(1);
        let out_h = (region.height.round() as i32).saturating_mul(scale).max(1);
        canvas
            .capture_region_surface(
                view_w,
                view_h,
                region,
                out_w,
                out_h,
                DrawOpts::export(transparent),
            )
            .ok()
    });

    // The file's own name seeds the suggested one, so a save lands as
    // `jw01783-o003_t009_nircam_clear-f187n_i2d.png` rather than `figure.png`.
    let title = std::path::Path::new(tab.source_file())
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    export_dialog::show(parent, &title, compose);
}
