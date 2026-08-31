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
use crate::ui::figure_plate::PlateContent;
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

    // A plate, not a bare crop: a figure that does not say what it shows or
    // where it points is a picture of some sky.
    let tab_for_plate = tab.clone();
    let compose: Compose = Rc::new(move |scale, transparent| {
        plate_content(&tab_for_plate, view_w, view_h, region).compose(scale, transparent)
    });
    let _ = &canvas;

    // The file's own name seeds the suggested one, so a save lands as
    // `jw01783-o003_t009_nircam_clear-f187n_i2d.png` rather than `figure.png`.
    export_dialog::show(parent, &file_title(tab), compose);
}

/// The plate for a region of a FITS image.
///
/// The cube's export has carried a title, a caption, a colour ramp and a footer
/// of facts since it was written; the FITS one came out as a bare crop with
/// none of it, so a figure could not say what it showed or where it pointed.
/// This is that plate, with what a FITS frame knows rather than what a cube
/// knows.
pub fn plate_content(
    tab: &Rc<FitsTab>,
    view_w: i32,
    view_h: i32,
    region: ViewRegion,
) -> PlateContent {
    use crate::ui::fits_viewer::{colormap_name, header_str, stretch_name};

    let data = tab.data();
    let header = &data.header;
    let sky = region_sky(tab, region);
    let unit = header_str(header, "BUNIT").unwrap_or_default();
    let with_unit = |v: f64| {
        if unit.is_empty() {
            format!("{v:.4}")
        } else {
            format!("{v:.4} {unit}")
        }
    };

    // What the figure is OF, on one line under the picture. Coordinates first,
    // because that is the question a reader asks of an astronomical image.
    let caption = match &sky {
        Some(s) => format!("{} \u{00B7} {}", s.centre, s.extent),
        None => crate::tr_en!("No WCS — pixel coordinates only").to_string(),
    };

    let mut footer: Vec<(String, String)> = vec![(
        crate::tr_en!("DIMENSIONS").to_string(),
        format!("{}\u{00D7}{}", data.width, data.height),
    )];
    if let Some(s) = &sky {
        footer.push((crate::tr_en!("RA").to_string(), s.ra_range.clone()));
        footer.push((crate::tr_en!("DEC").to_string(), s.dec_range.clone()));
        footer.push((crate::tr_en!("FIELD").to_string(), s.extent.clone()));
    }
    footer.push((
        crate::tr_en!("CUT LEVELS").to_string(),
        format!("{} … {}", with_unit(tab.vmin()), with_unit(tab.vmax())),
    ));
    footer.push((
        crate::tr_en!("STRETCH").to_string(),
        stretch_name(tab.stretch()).to_string(),
    ));
    for (key, card) in [
        (crate::tr_en!("OBJECT"), "OBJECT"),
        (crate::tr_en!("INSTRUMENT"), "INSTRUME"),
        (crate::tr_en!("FILTER"), "FILTER"),
    ] {
        if let Some(v) = header_str(header, card) {
            footer.push((key.to_string(), v));
        }
    }

    let canvas = tab.canvas().clone();
    PlateContent {
        capture: Rc::new(move |w, h| {
            let mut surf = canvas
                .capture_region_surface(view_w, view_h, region, w, h, DrawOpts::export(false))
                .ok()?;
            let (_, _, rgba) = crate::helpers::image_bytes::surface_to_rgba(&mut surf);
            Some(rgba)
        }),
        title: file_title(tab),
        subtitle: crate::tr_en!("FITS image").to_string(),
        caption,
        colormap: colormap_name(tab.colormap()).to_string(),
        ramp: crate::helpers::fits_renderer::build_lut(tab.colormap()),
        lo_label: with_unit(tab.vmin()),
        hi_label: with_unit(tab.vmax()),
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        footer,
        // None: a FITS frame's overlay — its marks and crosshair — is already in
        // the capture, drawn by the same function that draws the screen.
        overlay: None,
    }
}

/// The file's own name, which is what a figure should be called.
fn file_title(tab: &Rc<FitsTab>) -> String {
    std::path::Path::new(tab.source_file())
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// What a plate says about the region it shows.
///
/// The point of the caption: an exported figure that does not say where it is
/// pointing is a picture of some sky. These are read back through the same WCS
/// the crosshair uses, so the figure and the readout agree.
struct RegionSky {
    centre: String,
    extent: String,
    ra_range: String,
    dec_range: String,
}

/// Where `region` is on the sky, or `None` without a usable WCS.
fn region_sky(tab: &Rc<FitsTab>, region: ViewRegion) -> Option<RegionSky> {
    let data = tab.data();
    let wcs = data.wcs.as_ref().filter(|w| w.is_valid())?;
    let canvas = tab.canvas();
    // The region's four corners in IMAGE pixels, which is where the WCS lives.
    // Going through the canvas keeps North Up in the answer: a rotated view's
    // screen box is not an image-aligned box, and the corners are what say so.
    let corners = [
        canvas.screen_to_image_point_public(region.x, region.y),
        canvas.screen_to_image_point_public(region.x + region.width, region.y),
        canvas.screen_to_image_point_public(region.x + region.width, region.y + region.height),
        canvas.screen_to_image_point_public(region.x, region.y + region.height),
    ];
    let skies: Vec<(f64, f64)> = corners
        .iter()
        .map(|(x, y)| wcs.display_to_sky(*x, *y))
        .collect();
    let (cx, cy) = canvas.screen_to_image_point_public(
        region.x + region.width / 2.0,
        region.y + region.height / 2.0,
    );
    let (ra, dec) = wcs.display_to_sky(cx, cy);
    let (ra_s, dec_s) = WcsInfo::format_coords(ra, dec);

    let ras: Vec<f64> = skies.iter().map(|s| s.0).collect();
    let decs: Vec<f64> = skies.iter().map(|s| s.1).collect();
    let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    // The angular size of what is shown, from the region's size in image
    // pixels and this image's own scale.
    let per_px = wcs.pixel_scale_arcsec();
    let img_w = (corners[1].0 - corners[0].0).hypot(corners[1].1 - corners[0].1);
    let img_h = (corners[3].0 - corners[0].0).hypot(corners[3].1 - corners[0].1);
    let extent = if per_px.is_finite() && per_px > 0.0 {
        format!(
            "{} × {}",
            arcsec_text(img_w * per_px),
            arcsec_text(img_h * per_px)
        )
    } else {
        format!("{:.0} × {:.0} px", img_w, img_h)
    };

    Some(RegionSky {
        centre: format!("{ra_s} {dec_s}"),
        extent,
        ra_range: format!("{:.5}° … {:.5}°", lo(&ras), hi(&ras)),
        dec_range: format!("{:.5}° … {:.5}°", lo(&decs), hi(&decs)),
    })
}

/// An angle in the unit an astronomer would say it in.
fn arcsec_text(arcsec: f64) -> String {
    if arcsec >= 3600.0 {
        format!("{:.2}°", arcsec / 3600.0)
    } else if arcsec >= 60.0 {
        format!("{:.2}′", arcsec / 60.0)
    } else {
        format!("{arcsec:.2}″")
    }
}

/// Render `region` at `scale`, as the dialog's Save does.
///
/// Separate from the dialog wiring so it can be exercised without one: the
/// question "is the exported area the area that was dragged?" is about this
/// function, and nothing that needs a modal on screen.
pub fn compose_region(
    canvas: &Rc<crate::ui::fits_canvas::FitsCanvas>,
    view_w: i32,
    view_h: i32,
    region: ViewRegion,
    scale: i32,
    transparent: bool,
) -> Option<gtk4::cairo::ImageSurface> {
    // The output keeps the REGION's aspect, so a tall selection exports tall.
    // Deriving the height rather than taking it means the region is never
    // letterboxed into a frame of the wrong shape.
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
