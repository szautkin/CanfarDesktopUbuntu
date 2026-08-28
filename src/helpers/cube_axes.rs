//! Project the 3D volume box wireframe, the WCS axis captions, and the current
//! slice-plane quad onto screen (panel) coordinates, using the identical camera
//! matrix the GPU ray-marcher uses so the overlay aligns with the rendered
//! volume.
//!
//! One-to-one port of `Services/CubeViewer/CubeAxesOverlay.cs` (the Windows
//! analogue of the macOS `CubeAxisCaptions`). The reference builds the camera
//! matrices itself from az/el/dist; here the composed `view_proj`
//! (= `perspective * look_at`, **without** the box/model scale) is supplied by
//! the caller, and we apply the box scale to the model-space corners exactly as
//! `CubeAxesOverlay` does inside its local `Project` helper.
//!
//! The spatial-endpoint formatters (`RA`/`DEC` sexagesimal, galactic degrees)
//! live here because the Verbinal [`CubeWcs`] surface exposes only the spectral
//! helpers; the spatial caption text is produced from [`WcsInfo::pixel_to_sky`].

use crate::helpers::cube_math::Mat4;
use crate::helpers::cube_wcs::CubeWcs;

/// Behind-the-near-plane threshold on clip-space `w` (matches the C# `1e-4f`).
const W_EPS: f32 = 1e-4;

/// The 8 unit-box corners in model space (±0.5), ported verbatim.
const CORNERS: [(f32, f32, f32); 8] = [
    (-0.5, -0.5, -0.5),
    (0.5, -0.5, -0.5),
    (0.5, 0.5, -0.5),
    (-0.5, 0.5, -0.5),
    (-0.5, -0.5, 0.5),
    (0.5, -0.5, 0.5),
    (0.5, 0.5, 0.5),
    (-0.5, 0.5, 0.5),
];

/// The 12 edges as corner-index pairs (back face, front face, connectors).
const EDGE_INDICES: [(usize, usize); 12] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 0), // back face  (z = -0.5)
    (4, 5),
    (5, 6),
    (6, 7),
    (7, 4), // front face (z = +0.5)
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7), // connecting edges
];

/// Projected overlay for one camera frame. Screen coordinates are in panel
/// pixels (y-down). Points behind the camera are dropped, so an edge/caption is
/// present only when it is actually drawable; `slice_quad` holds the 4 plane
/// corners when all four are visible and is empty otherwise.
#[derive(Debug, Clone, Default)]
pub struct AxesOverlay {
    /// The projected box edges (up to 12), each an endpoint pair.
    pub edges: Vec<((f32, f32), (f32, f32))>,
    /// Axis-name + endpoint-value captions (up to 9): `(x, y, text)`.
    pub captions: Vec<(f32, f32, String)>,
    /// The 4 projected corners of the current-channel slice plane, or empty.
    pub slice_quad: Vec<(f32, f32)>,
}

/// Everything [`build`] needs to lay out the axes box for one frame.
///
/// A struct rather than nine positional arguments: the two `(usize, usize)`
/// pairs and the `f32` pair are easy to transpose at a call site, and a
/// transposed width/height silently produces a skewed box instead of an error.
#[derive(Clone, Copy)]
pub struct AxesRequest<'a> {
    /// Rendered volume dimensions `(nx, ny, nz)` — drive the box aspect and the
    /// spectral endpoint channels.
    pub dims: (usize, usize, usize),
    /// Cube WCS for the captions.
    pub wcs: &'a CubeWcs,
    /// `perspective * look_at` (column-major, OpenGL clip space); the box/model
    /// scale is applied inside `build`, not baked into this matrix.
    pub view_proj: &'a Mat4,
    /// Panel size in pixels, `(width, height)`.
    pub panel: (f32, f32),
    /// Current channel index for the slice-plane marker.
    pub slice_z: usize,
    /// Z-axis box stretch (the spectral-scale control).
    pub spectral_scale: f32,
}

/// Build the projected wireframe + captions + slice-plane for one frame.
pub fn build(req: &AxesRequest) -> AxesOverlay {
    let AxesRequest {
        dims: (nx, ny, nz),
        wcs,
        view_proj,
        panel: (panel_w, panel_h),
        slice_z,
        spectral_scale,
    } = *req;
    let mut out = AxesOverlay::default();
    if panel_w < 1.0 || panel_h < 1.0 {
        return out;
    }

    // Box (model) scale: spatial aspect from nx/ny, spectral axis from the caller's
    // spectral_scale — identical to CubeAxesOverlay's `sx/sy/sz` and the GL model.
    let [sx, sy, sz] = box_scale(req.dims, spectral_scale);

    let proj = |bx: f32, by: f32, bz: f32| {
        project(view_proj, [bx * sx, by * sy, bz * sz], panel_w, panel_h)
    };

    // Box wireframe.
    let mut corners: [Option<(f32, f32)>; 8] = [None; 8];
    for (i, &(x, y, z)) in CORNERS.iter().enumerate() {
        corners[i] = proj(x, y, z);
    }
    for &(a, b) in EDGE_INDICES.iter() {
        if let (Some(pa), Some(pb)) = (corners[a], corners[b]) {
            out.edges.push((pa, pb));
        }
    }

    // Slice-plane marker: a quad across the box at the current channel's depth
    // (model Z = -0.5 + fraction). Only emitted when all 4 corners are visible.
    let frac = if nz > 1 {
        slice_z.min(nz - 1) as f32 / (nz - 1) as f32
    } else {
        0.0
    };
    let z = -0.5 + frac.clamp(0.0, 1.0);
    let quad = [
        proj(-0.5, -0.5, z),
        proj(0.5, -0.5, z),
        proj(0.5, 0.5, z),
        proj(-0.5, 0.5, z),
    ];
    if quad.iter().all(Option::is_some) {
        for p in quad {
            out.slice_quad.push(p.unwrap());
        }
    }

    // WCS axis captions — positions in box space exactly as CubeAxesOverlay.
    let galactic = spatial_galactic(wcs);
    let lon_name = if galactic { "GLON" } else { "RA" };
    let lat_name = if galactic { "GLAT" } else { "DEC" };
    let x_hi = nx.saturating_sub(1);
    let y_hi = ny.saturating_sub(1);
    let z_hi = nz.saturating_sub(1);

    // X axis (longitude): name below-front; endpoints at ±0.5 in X.
    add_caption(
        &mut out.captions,
        proj(0.0, -0.62, -0.62),
        lon_name.to_string(),
    );
    add_caption(
        &mut out.captions,
        proj(-0.5, -0.62, -0.62),
        lon_text(wcs, galactic, 0, ny),
    );
    add_caption(
        &mut out.captions,
        proj(0.5, -0.62, -0.62),
        lon_text(wcs, galactic, x_hi, ny),
    );
    // Y axis (latitude).
    add_caption(
        &mut out.captions,
        proj(-0.62, 0.0, -0.62),
        lat_name.to_string(),
    );
    add_caption(
        &mut out.captions,
        proj(-0.62, -0.5, -0.62),
        lat_text(wcs, galactic, 0, nx),
    );
    add_caption(
        &mut out.captions,
        proj(-0.62, 0.5, -0.62),
        lat_text(wcs, galactic, y_hi, nx),
    );
    // Z axis (spectral).
    add_caption(
        &mut out.captions,
        proj(-0.62, -0.62, 0.0),
        spectral_name(wcs, nz),
    );
    add_caption(
        &mut out.captions,
        proj(-0.62, -0.62, -0.5),
        spec_text(wcs, 0),
    );
    add_caption(
        &mut out.captions,
        proj(-0.62, -0.62, 0.5),
        spec_text(wcs, z_hi),
    );

    out
}

/// The model scale of the box for a cube of `dims`.
///
/// Spatial aspect from `nx`/`ny`, spectral from the caller. Shared by the axes
/// box and by anything else that needs to put a point in the same space — the
/// alternative is two copies of this arithmetic that agree until one is
/// changed, and a mark that no longer sits where its voxel is.
pub fn box_scale(dims: (usize, usize, usize), spectral_scale: f32) -> [f32; 3] {
    let (nx, ny, _) = dims;
    let m = nx.max(ny) as f32;
    let m = if m <= 0.0 { 1.0 } else { m };
    [nx as f32 / m, ny as f32 / m, spectral_scale]
}

/// A voxel's position in unscaled box space, each axis in `-0.5..=0.5`.
///
/// The same convention the slice-plane marker uses (`-0.5 + index/(n-1)`), and
/// deliberately so: an annotation at channel 40 must land on the plane the
/// viewer draws for channel 40.
pub fn voxel_to_box(voxel: (f64, f64, f64), dims: (usize, usize, usize)) -> [f32; 3] {
    let axis = |v: f64, n: usize| -> f32 {
        if n <= 1 {
            return 0.0;
        }
        (-0.5 + (v / (n - 1) as f64)).clamp(-0.5, 0.5) as f32
    };
    let (nx, ny, nz) = dims;
    [axis(voxel.0, nx), axis(voxel.1, ny), axis(voxel.2, nz)]
}

/// Where a voxel falls on the panel, or `None` when it is behind the near plane.
///
/// The projection annotations use. It goes through the same `project` as the
/// box and the captions, so a mark cannot drift away from the wireframe it is
/// drawn against.
pub fn project_voxel(
    view_proj: &Mat4,
    dims: (usize, usize, usize),
    spectral_scale: f32,
    voxel: (f64, f64, f64),
    panel: (f32, f32),
) -> Option<(f32, f32)> {
    let b = voxel_to_box(voxel, dims);
    let s = box_scale(dims, spectral_scale);
    project(
        view_proj,
        [b[0] * s[0], b[1] * s[1], b[2] * s[2]],
        panel.0,
        panel.1,
    )
}

/// Project a model-space point (already box-scaled) through the column-major
/// `view_proj` to panel pixels. Returns `None` when the point is on/behind the
/// near plane (`w <= W_EPS`), matching CubeAxesOverlay's visibility test.
///
/// Done inline (rather than via `cube_math::transform_point`) because we need
/// clip-space `w` for the near-plane cull, which a `[f32;3]` result discards.
fn project(vp: &Mat4, p: [f32; 3], panel_w: f32, panel_h: f32) -> Option<(f32, f32)> {
    let (x, y, z) = (p[0], p[1], p[2]);
    // Column-major mat4 * (x, y, z, 1): column c lives at indices [4c..4c+4].
    let cw = vp[3] * x + vp[7] * y + vp[11] * z + vp[15];
    if cw <= W_EPS {
        return None;
    }
    let cx = vp[0] * x + vp[4] * y + vp[8] * z + vp[12];
    let cy = vp[1] * x + vp[5] * y + vp[9] * z + vp[13];
    let ndc_x = cx / cw;
    let ndc_y = cy / cw;
    let px = (ndc_x * 0.5 + 0.5) * panel_w;
    // clip-space y-up → screen y-down
    let py = (1.0 - (ndc_y * 0.5 + 0.5)) * panel_h;
    Some((px, py))
}

fn add_caption(caps: &mut Vec<(f32, f32, String)>, at: Option<(f32, f32)>, text: String) {
    if let Some((x, y)) = at {
        caps.push((x, y, text));
    }
}

// ── Spatial captions ───────────────────────────────────────────────────────

/// True when the spatial frame is galactic (CTYPE1 = GLON-…).
fn spatial_galactic(wcs: &CubeWcs) -> bool {
    wcs.spatial
        .as_ref()
        .is_some_and(|s| s.ctype1.trim().to_ascii_uppercase().starts_with("GLON"))
}

/// Formatted longitude value at a 0-based X pixel, evaluated at the cube's mid Y
/// (port of `CubeWcs.LonText`). Falls back to `px N` without a valid WCS.
fn lon_text(wcs: &CubeWcs, galactic: bool, pix_x0: usize, ny: usize) -> String {
    match wcs.spatial.as_ref().filter(|s| s.is_valid()) {
        Some(s) => {
            let (lon, _lat) = s.pixel_to_sky(pix_x0 as f64 + 1.0, ny as f64 / 2.0);
            if galactic {
                // Linear/CAR galactic longitude is not wrapped; fold into [0,360).
                format_deg(wrap360(lon))
            } else {
                format_ra_short(lon)
            }
        }
        None => format!("px {}", pix_x0),
    }
}

/// Formatted latitude value at a 0-based Y pixel, evaluated at the cube's mid X
/// (port of `CubeWcs.LatText`). Falls back to `px N` without a valid WCS.
fn lat_text(wcs: &CubeWcs, galactic: bool, pix_y0: usize, nx: usize) -> String {
    match wcs.spatial.as_ref().filter(|s| s.is_valid()) {
        Some(s) => {
            let (_lon, lat) = s.pixel_to_sky(nx as f64 / 2.0, pix_y0 as f64 + 1.0);
            if galactic {
                format_deg(lat)
            } else {
                format_dec_short(lat)
            }
        }
        None => format!("px {}", pix_y0),
    }
}

// ── Spectral captions ──────────────────────────────────────────────────────

/// Human axis name + display unit for the spectral axis (e.g. "FREQUENCY GHz"),
/// or "CHANNEL" when the third axis is not a usable spectral WCS.
fn spectral_name(wcs: &CubeWcs, nz: usize) -> String {
    match wcs.spectral.as_ref() {
        Some(s) if s.cdelt3 != 0.0 && nz > 1 => {
            let unit = wcs
                .channel_to_physical(0.0)
                .map(|(_, u)| u)
                .unwrap_or_default();
            format!("{} {}", spectral_axis_name(&s.ctype3), unit)
                .trim()
                .to_string()
        }
        _ => "CHANNEL".to_string(),
    }
}

/// Formatted spectral value at a 0-based channel (port of `CubeWcs.SpecText`).
fn spec_text(wcs: &CubeWcs, ch: usize) -> String {
    match wcs.channel_to_physical(ch as f64) {
        Some((v, _unit)) => fmt_g3(v),
        None => format!("CH {}", ch),
    }
}

/// Human axis name from a CTYPE3 stem (`FREQ` → `FREQUENCY`, …). Port of
/// `CubeWcs.SpecAxisName`'s switch.
fn spectral_axis_name(ctype3: &str) -> String {
    let t = ctype3.trim();
    // Stem = token before the first '-' (only when the dash is not leading).
    let stem = match t.find('-') {
        Some(d) if d > 0 => &t[..d],
        _ => t,
    };
    match stem.to_ascii_uppercase().as_str() {
        "FREQ" => "FREQUENCY".to_string(),
        "VRAD" | "VELO" | "VOPT" => "VELOCITY".to_string(),
        "WAVE" | "AWAV" => "WAVELENGTH".to_string(),
        "WAVN" => "WAVENUMBER".to_string(),
        "FDEP" => "FARADAY DEPTH".to_string(),
        _ => {
            if t.is_empty() {
                "SPECTRAL".to_string()
            } else {
                t.to_ascii_uppercase()
            }
        }
    }
}

// ── Compact sexagesimal / decimal formatters (ported from CubeWcs.cs) ───────

/// `raDeg` → `"HH:MM:SS"` (RA folded into [0,24h)).
fn format_ra_short(ra_deg: f64) -> String {
    let mut ra = ra_deg / 15.0;
    ra %= 24.0;
    if ra < 0.0 {
        ra += 24.0;
    }
    let mut h = ra as i32;
    let mut m = ((ra - h as f64) * 60.0) as i32;
    let mut s = ((ra - h as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        h = (h + 1) % 24;
    }
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// `decDeg` → `"±DD:MM:SS"` (uses U+2212 MINUS SIGN for negatives, as the C#).
fn format_dec_short(dec_deg: f64) -> String {
    let sign = if dec_deg >= 0.0 { "+" } else { "\u{2212}" };
    let d = dec_deg.abs();
    let mut dd = d as i32;
    let mut m = ((d - dd as f64) * 60.0) as i32;
    let mut s = ((d - dd as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        dd += 1;
    }
    format!("{}{:02}:{:02}:{:02}", sign, dd, m, s)
}

/// Decimal degrees to 3 places with a trailing degree sign.
fn format_deg(deg: f64) -> String {
    format!("{:.3}\u{00B0}", deg)
}

/// Fold an angle into [0, 360).
fn wrap360(v: f64) -> f64 {
    ((v % 360.0) + 360.0) % 360.0
}

/// Mimic .NET's `"0.###"`: up to 3 fractional digits, trailing zeros trimmed.
fn fmt_g3(v: f64) -> String {
    let s = format!("{:.3}", v);
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" || trimmed == "\u{2212}" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An [`AxesRequest`] over the identity camera at the reference spectral
    /// scale — the two constants every test here shares.
    fn req<'a>(
        dims: (usize, usize, usize),
        wcs: &'a CubeWcs,
        panel: (f32, f32),
        slice_z: usize,
    ) -> AxesRequest<'a> {
        AxesRequest {
            dims,
            wcs,
            view_proj: &IDENTITY,
            panel,
            slice_z,
            spectral_scale: 1.5,
        }
    }

    const IDENTITY: Mat4 = [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];

    #[test]
    fn project_center_and_offset() {
        // With identity view_proj, w == 1 so NDC == the input xy.
        assert_eq!(
            project(&IDENTITY, [0.0, 0.0, 0.0], 100.0, 100.0),
            Some((50.0, 50.0))
        );
        // +x,+y → right and (y-down) up.
        let p = project(&IDENTITY, [0.5, 0.5, 0.0], 100.0, 100.0).unwrap();
        assert!((p.0 - 75.0).abs() < 1e-4, "px={}", p.0);
        assert!((p.1 - 25.0).abs() < 1e-4, "py={}", p.1);
    }

    #[test]
    fn project_culls_behind_near_plane() {
        // A w-row that produces a non-positive clip w must be culled.
        let mut vp = IDENTITY;
        vp[15] = -1.0; // w = -1 for the origin
        assert_eq!(project(&vp, [0.0, 0.0, 0.0], 100.0, 100.0), None);
    }

    #[test]
    fn build_wireframe_slice_and_captions() {
        let wcs = CubeWcs::from_header(&HashMap::new());
        let overlay = build(&req((10, 10, 10), &wcs, (200.0, 200.0), 5));
        // Identity keeps every corner in front → all 12 edges, 9 captions, quad.
        assert_eq!(overlay.edges.len(), 12);
        assert_eq!(overlay.captions.len(), 9);
        assert_eq!(overlay.slice_quad.len(), 4);
    }

    #[test]
    fn build_empty_on_degenerate_panel() {
        let wcs = CubeWcs::from_header(&HashMap::new());
        let overlay = build(&req((4, 4, 4), &wcs, (0.0, 100.0), 0));
        assert!(overlay.edges.is_empty());
        assert!(overlay.captions.is_empty());
        assert!(overlay.slice_quad.is_empty());
    }

    #[test]
    fn spatial_fallback_captions_without_wcs() {
        // No WCS → "RA"/"DEC" axis names and "px N" endpoint values, "CH N" spectral.
        let wcs = CubeWcs::from_header(&HashMap::new());
        let overlay = build(&req((8, 6, 4), &wcs, (200.0, 200.0), 0));
        let texts: Vec<&str> = overlay.captions.iter().map(|c| c.2.as_str()).collect();
        assert!(texts.contains(&"RA"));
        assert!(texts.contains(&"DEC"));
        assert!(texts.contains(&"CHANNEL"));
        assert!(texts.contains(&"px 0"));
        assert!(texts.contains(&"px 7")); // nx-1
        assert!(texts.contains(&"px 5")); // ny-1
        assert!(texts.contains(&"CH 0"));
        assert!(texts.contains(&"CH 3")); // nz-1
    }

    #[test]
    fn ra_dec_formatters() {
        assert_eq!(format_ra_short(180.0), "12:00:00");
        assert_eq!(format_ra_short(0.0), "00:00:00");
        assert_eq!(format_ra_short(-15.0), "23:00:00"); // wraps into [0,24h)
        assert_eq!(format_dec_short(45.0), "+45:00:00");
        assert_eq!(format_dec_short(-30.5), "\u{2212}30:30:00");
    }

    #[test]
    fn degree_and_wrap_helpers() {
        assert_eq!(format_deg(12.0), "12.000\u{00B0}");
        assert!((wrap360(-10.0) - 350.0).abs() < 1e-9);
        assert!((wrap360(370.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn g3_formatting() {
        assert_eq!(fmt_g3(1.4204057), "1.42");
        assert_eq!(fmt_g3(1000.0), "1000");
        assert_eq!(fmt_g3(0.0), "0");
        assert_eq!(fmt_g3(-1.5), "-1.5");
    }

    #[test]
    fn spectral_axis_names() {
        assert_eq!(spectral_axis_name("FREQ-LSR"), "FREQUENCY");
        assert_eq!(spectral_axis_name("VRAD"), "VELOCITY");
        assert_eq!(spectral_axis_name("VELO-LSR"), "VELOCITY");
        assert_eq!(spectral_axis_name("WAVE"), "WAVELENGTH");
        assert_eq!(spectral_axis_name("FDEP"), "FARADAY DEPTH");
        assert_eq!(spectral_axis_name(""), "SPECTRAL");
        assert_eq!(spectral_axis_name("STOKES"), "STOKES");
    }
}

#[cfg(test)]
mod voxel_projection_tests {
    use super::*;

    fn dims() -> (usize, usize, usize) {
        (64, 64, 24)
    }

    /// A voxel maps into the box the same way the slice marker does.
    ///
    /// This is the guard that matters. The slice-plane quad puts channel `c` at
    /// model `z = -0.5 + c/(nz-1)`; if an annotation used any other convention
    /// it would sit near the right plane and not on it, which looks like a
    /// rendering imprecision rather than a wrong formula.
    #[test]
    fn a_voxel_lands_on_the_plane_the_viewer_draws_for_its_channel() {
        let (_, _, nz) = dims();
        for channel in [0usize, 7, 12, nz - 1] {
            let expected_z = -0.5 + channel as f32 / (nz - 1) as f32;
            let b = voxel_to_box((32.0, 32.0, channel as f64), dims());
            assert!(
                (b[2] - expected_z).abs() < 1e-6,
                "channel {channel} mapped to z {} not {expected_z}",
                b[2]
            );
        }
    }

    /// The centre voxel is the centre of the box.
    #[test]
    fn the_middle_voxel_is_the_middle_of_the_box() {
        let (nx, ny, nz) = dims();
        let b = voxel_to_box(
            (
                (nx - 1) as f64 / 2.0,
                (ny - 1) as f64 / 2.0,
                (nz - 1) as f64 / 2.0,
            ),
            dims(),
        );
        for c in b {
            assert!(c.abs() < 1e-6, "centre voxel mapped to {b:?}");
        }
    }

    /// Corners map to corners.
    #[test]
    fn the_first_and_last_voxels_are_the_box_corners() {
        let (nx, ny, nz) = dims();
        let low = voxel_to_box((0.0, 0.0, 0.0), dims());
        let high = voxel_to_box(((nx - 1) as f64, (ny - 1) as f64, (nz - 1) as f64), dims());
        assert_eq!(low, [-0.5, -0.5, -0.5]);
        assert_eq!(high, [0.5, 0.5, 0.5]);
    }

    /// A voxel outside the cube is clamped to it, not projected into space.
    #[test]
    fn an_out_of_range_voxel_is_clamped_to_the_box() {
        let b = voxel_to_box((-40.0, 900.0, 900.0), dims());
        for c in b {
            assert!((-0.5..=0.5).contains(&c), "{b:?} escaped the box");
        }
    }

    /// A degenerate axis does not divide by zero.
    #[test]
    fn a_single_channel_cube_does_not_divide_by_zero() {
        let b = voxel_to_box((0.0, 0.0, 0.0), (64, 64, 1));
        assert!(b.iter().all(|c| c.is_finite()), "{b:?}");
        assert_eq!(b[2], 0.0, "a one-channel cube has no depth to place on");
    }

    /// The scale used for annotations is the scale the box is drawn with.
    #[test]
    fn the_shared_scale_matches_the_boxs_own() {
        let s = box_scale((80, 40, 10), 0.7);
        assert!(
            (s[0] - 1.0).abs() < 1e-6,
            "the long axis should be 1.0: {s:?}"
        );
        assert!((s[1] - 0.5).abs() < 1e-6, "{s:?}");
        assert!(
            (s[2] - 0.7).abs() < 1e-6,
            "the spectral scale is the caller's: {s:?}"
        );
        // A degenerate cube must not produce a NaN scale.
        assert!(box_scale((0, 0, 0), 1.0).iter().all(|c| c.is_finite()));
    }
}
