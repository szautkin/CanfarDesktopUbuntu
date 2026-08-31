//! Save what the FITS viewer is showing — or part of it — as PNG or PDF.
//!
//! The dialog is [`crate::ui::export_dialog`], shared with the cube viewer.
//! All this module does is turn a tab and a region into the [`Compose`] that
//! dialog asks for: pixels at a scale, with or without a ground.
//!
//! Its own file rather than more of `fits_viewer`, which is already the
//! largest in the tree.

use crate::models::fits_image::WcsInfo;
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

/// Turn a `region` argument into a rectangle of the view.
///
/// The four forms an agent can ask in, and why they are not all the same
/// thing: a dragged region is in SCREEN space, while an agent's is in image
/// pixels or on the sky, and on a north-up frame those are not the same
/// rectangle. Image and sky boxes are therefore mapped corner by corner
/// through `image_to_screen_point`, which knows about the rotation, and the
/// screen-aligned bounding box of the four corners is taken — never smaller
/// than what was asked for.
///
/// `None` means the argument was malformed, which is worth an error rather
/// than a silent fall back to the whole view: an agent that asked for a region
/// and got the frame would describe the wrong picture confidently.
pub fn resolve_region(
    tab: &Rc<FitsTab>,
    arg: Option<&serde_json::Value>,
    view_w: i32,
    view_h: i32,
) -> Result<ViewRegion, String> {
    let whole = ViewRegion::whole(view_w, view_h);
    let Some(arg) = arg else {
        return Ok(whole);
    };
    if let Some(name) = arg.as_str() {
        return match name.trim().to_ascii_lowercase().as_str() {
            "view" => Ok(whole),
            "image" => Ok(image_box(
                tab,
                0.0,
                0.0,
                tab.data().width as f64,
                tab.data().height as f64,
            )),
            other => Err(format!(
                "'{other}' is not a region — use \"view\", \"image\", or a box"
            )),
        };
    }
    let num = |k: &str| arg.get(k).and_then(|v| v.as_f64());
    if let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("width"), num("height"))
    {
        if w <= 0.0 || h <= 0.0 {
            return Err("a region needs a positive width and height".to_string());
        }
        return Ok(image_box(tab, x, y, w, h));
    }
    if let (Some(ra), Some(dec), Some(aw), Some(ah)) = (
        num("ra"),
        num("dec"),
        num("widthArcsec"),
        num("heightArcsec"),
    ) {
        let wcs = tab
            .data()
            .wcs
            .as_ref()
            .filter(|w| w.is_valid())
            .ok_or_else(|| {
                "this image has no WCS, so a region cannot be given on the sky — \
                 use x/y/width/height in image pixels"
                    .to_string()
            })?;
        return sky_box(tab, wcs, ra, dec, aw, ah);
    }
    Err(
        "a region is \"view\", \"image\", {x,y,width,height} in image pixels, or \
         {ra,dec,widthArcsec,heightArcsec} on the sky"
            .to_string(),
    )
}

/// The screen rectangle covering an image-space box.
///
/// Corner by corner, because North Up rotates the image: an image-aligned box
/// is not screen-aligned, and mapping only two corners would clip the region
/// on a rotated frame.
fn image_box(tab: &Rc<FitsTab>, x: f64, y: f64, w: f64, h: f64) -> ViewRegion {
    let canvas = tab.canvas();
    let corners = [
        canvas.image_to_screen_point(x, y),
        canvas.image_to_screen_point(x + w, y),
        canvas.image_to_screen_point(x + w, y + h),
        canvas.image_to_screen_point(x, y + h),
    ];
    let (xs, ys): (Vec<f64>, Vec<f64>) = corners.iter().copied().unzip();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    ViewRegion::between((min(&xs), min(&ys)), (max(&xs), max(&ys)))
}

/// The same, for a box stated on the sky.
fn sky_box(
    tab: &Rc<FitsTab>,
    wcs: &WcsInfo,
    ra: f64,
    dec: f64,
    width_arcsec: f64,
    height_arcsec: f64,
) -> Result<ViewRegion, String> {
    if width_arcsec <= 0.0 || height_arcsec <= 0.0 {
        return Err("a sky region needs a positive width and height".to_string());
    }
    let (cx, cy) = wcs
        .sky_to_display(ra, dec)
        .ok_or_else(|| format!("RA {ra}, Dec {dec} is not on this image"))?;
    // Arcseconds to image pixels through this image's own scale.
    let per_px = wcs.pixel_scale_arcsec();
    // NaN as well as zero: an image with no scale gives NaN here, and a NaN
    // half-width would produce a region of nothing.
    if !per_px.is_finite() || per_px <= 0.0 {
        return Err("this image has no usable pixel scale".to_string());
    }
    let (hw, hh) = (width_arcsec / per_px / 2.0, height_arcsec / per_px / 2.0);
    Ok(image_box(tab, cx - hw, cy - hh, hw * 2.0, hh * 2.0))
}
