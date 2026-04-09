# 06 - FITS Viewer Module Specification

## Purpose

The FITS Viewer module displays astronomical FITS images with interactive pan/zoom, stretch and colormap controls, WCS coordinate readout, linked crosshairs across tabs, blink comparison, and a searchable header panel. It uses a pure Rust FITS parser (no C library dependency in the default build) with an optional `cfitsio` backend.

## Architecture

```
FitsTabHost (singleton, lives in ViewStack as "fits" page)
    |
    +-- Tab 1: FitsViewerPage
    |       |-- FitsControls (stretch, colormap, min/max)
    |       |-- FitsCanvas (DrawingArea + zoom/pan/crosshair)
    |       |-- HeaderPanel (keyword search + list)
    |       +-- FitsImageData + WcsInfo
    |
    +-- Tab 2: FitsViewerPage
    |       |-- ...
    |
    +-- Shared State:
            |-- shared_cursor: Rc<RefCell<Option<(f64, f64)>>>
            |-- saved_coordinates: Rc<RefCell<Vec<SavedCoordinate>>>
```

### FitsTabHost (FitsViewer)

Existing implementation at `src/ui/fits_viewer.rs`. Singleton registered in `ViewStack` under name `"fits"`.

```rust
pub struct FitsViewer {
    widget: gtk::Box,
    notebook: gtk::Notebook,
    tabs: Rc<RefCell<Vec<Rc<FitsTab>>>>,
    shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
    status_label: gtk::Label,
    blink_active: Rc<RefCell<bool>>,
    saved_coordinates: Rc<RefCell<Vec<SavedCoordinate>>>,
}
```

Toolbar buttons:
- **Open FITS** (`document-open-symbolic`, `suggested-action`): File chooser for `.fits`, `.fit`, `.fts`, `.FITS`.
- **Blink** (`view-refresh-symbolic`, `ToggleButton`): Toggle blink comparison between first two tabs.
- **Sync Zoom** (`zoom-fit-best-symbolic`, `ToggleButton`): Match angular extent across tabs.
- **North Up** (`find-location-symbolic`): Rotate current tab so celestial north points up.
- **Saved Coords** (`starred-symbolic`): Open saved coordinates popover.
- Spacer.
- **Status label**: Shows FITS summary (dimensions, object, telescope, instrument, date, exposure, WCS availability).

## FITS Parser (Pure Rust)

### Overview

Read FITS files without any C library dependency. Located at `src/helpers/fits_loader.rs` (pure implementation) alongside the existing `cfitsio`-based loader (behind `#[cfg(feature = "fits")]`).

The pure parser is the default. The `cfitsio` backend is available behind the `fits` feature flag for users who need advanced HDU types.

### Block Structure

FITS files are composed of 2880-byte blocks. Each HDU (Header Data Unit) consists of header blocks followed by data blocks.

```rust
const FITS_BLOCK_SIZE: usize = 2880;
const FITS_CARD_SIZE: usize = 80;
const CARDS_PER_BLOCK: usize = FITS_BLOCK_SIZE / FITS_CARD_SIZE;  // 36

pub fn load_fits_pure(path: &Path) -> Result<FitsFile, String> {
    let data = std::fs::read(path)
        .map_err(|e| format!("Cannot read file: {}", e))?;
    
    // Safety cap: refuse files > 512 MB
    if data.len() > 512 * 1024 * 1024 {
        return Err("FITS file exceeds 512 MB size limit".to_string());
    }
    
    let mut offset = 0;
    let mut hdus = Vec::new();
    
    while offset < data.len() {
        let hdu = parse_hdu(&data, &mut offset)?;
        hdus.push(hdu);
    }
    
    Ok(FitsFile { hdus })
}
```

### Header Parsing

```rust
pub struct FitsCard {
    pub keyword: String,    // First 8 characters, trimmed
    pub value: String,      // After '= ', parsed (string quotes removed, numbers trimmed)
    pub comment: String,    // After '/' separator
    pub raw: String,        // Full 80-character card image
}

pub struct FitsHeader {
    pub cards: Vec<FitsCard>,
}

impl FitsHeader {
    pub fn get(&self, keyword: &str) -> Option<&str>;
    pub fn get_f64(&self, keyword: &str) -> Option<f64>;
    pub fn get_i64(&self, keyword: &str) -> Option<i64>;
    pub fn get_bool(&self, keyword: &str) -> Option<bool>;
}
```

Parse 80-character cards from 2880-byte blocks:

```rust
fn parse_header(data: &[u8], offset: &mut usize) -> Result<FitsHeader, String> {
    let mut cards = Vec::new();
    
    loop {
        if *offset + FITS_CARD_SIZE > data.len() {
            return Err("Unexpected end of file in header".to_string());
        }
        
        let card_bytes = &data[*offset..*offset + FITS_CARD_SIZE];
        let card_str = String::from_utf8_lossy(card_bytes).to_string();
        *offset += FITS_CARD_SIZE;
        
        let keyword = card_str[..8].trim().to_string();
        
        if keyword == "END" {
            // Advance offset to next 2880-byte boundary
            let remainder = *offset % FITS_BLOCK_SIZE;
            if remainder != 0 {
                *offset += FITS_BLOCK_SIZE - remainder;
            }
            break;
        }
        
        let (value, comment) = parse_card_value(&card_str);
        cards.push(FitsCard {
            keyword,
            value,
            comment,
            raw: card_str,
        });
    }
    
    Ok(FitsHeader { cards })
}
```

Card value parsing rules:
- If characters 8-9 are `"= "`: value field starts at character 10.
- String values: enclosed in single quotes, strip quotes, trim trailing spaces inside quotes.
- Logical values: `T` or `F` at character 30.
- Numeric values: trim whitespace, parse as integer or float.
- Comment separator: first `/` outside a quoted string.
- `CONTINUE` keyword: continuation of a long string value from the previous card.
- `COMMENT` and `HISTORY` keywords: no value, entire card after keyword is comment text.

### Data Parsing

```rust
fn parse_image_data(
    data: &[u8],
    offset: &mut usize,
    header: &FitsHeader,
) -> Result<Option<Vec<f64>>, String> {
    let naxis = header.get_i64("NAXIS").unwrap_or(0) as usize;
    if naxis < 2 {
        return Ok(None);  // No image data
    }
    
    let naxis1 = header.get_i64("NAXIS1").unwrap_or(0) as usize;  // width
    let naxis2 = header.get_i64("NAXIS2").unwrap_or(0) as usize;  // height
    let bitpix = header.get_i64("BITPIX").unwrap_or(0);
    let bscale = header.get_f64("BSCALE").unwrap_or(1.0);
    let bzero = header.get_f64("BZERO").unwrap_or(0.0);
    
    let npixels = naxis1 * naxis2;
    let bytes_per_pixel = (bitpix.abs() / 8) as usize;
    let data_size = npixels * bytes_per_pixel;
    
    if *offset + data_size > data.len() {
        return Err("Unexpected end of file in image data".to_string());
    }
    
    let raw = &data[*offset..*offset + data_size];
    let mut pixels = Vec::with_capacity(npixels);
    
    for i in 0..npixels {
        let start = i * bytes_per_pixel;
        let raw_val = match bitpix {
            8 => raw[start] as f64,
            16 => i16::from_be_bytes([raw[start], raw[start + 1]]) as f64,
            32 => i32::from_be_bytes([raw[start], raw[start + 1], raw[start + 2], raw[start + 3]]) as f64,
            -32 => f32::from_be_bytes([raw[start], raw[start + 1], raw[start + 2], raw[start + 3]]) as f64,
            -64 => f64::from_be_bytes([
                raw[start], raw[start + 1], raw[start + 2], raw[start + 3],
                raw[start + 4], raw[start + 5], raw[start + 6], raw[start + 7],
            ]),
            _ => return Err(format!("Unsupported BITPIX: {}", bitpix)),
        };
        pixels.push(raw_val * bscale + bzero);
    }
    
    // Y-flip: FITS convention is bottom-to-top, screen is top-to-bottom
    let mut flipped = vec![0.0; npixels];
    for y in 0..naxis2 {
        let src_row = (naxis2 - 1 - y) * naxis1;
        let dst_row = y * naxis1;
        flipped[dst_row..dst_row + naxis1].copy_from_slice(&pixels[src_row..src_row + naxis1]);
    }
    
    // Advance offset past data, align to 2880-byte boundary
    *offset += data_size;
    let remainder = *offset % FITS_BLOCK_SIZE;
    if remainder != 0 {
        *offset += FITS_BLOCK_SIZE - remainder;
    }
    
    Ok(Some(flipped))
}
```

BITPIX values and their meaning:
| BITPIX | Type | Bytes | Rust Type |
|--------|------|-------|-----------|
| 8 | Unsigned byte | 1 | `u8` |
| 16 | 16-bit signed integer | 2 | `i16` |
| 32 | 32-bit signed integer | 4 | `i32` |
| -32 | 32-bit IEEE float | 4 | `f32` |
| -64 | 64-bit IEEE float | 8 | `f64` |

All multi-byte values are big-endian (FITS standard).

### FitsHdu Model

```rust
pub struct FitsFile {
    pub hdus: Vec<FitsHdu>,
}

pub struct FitsHdu {
    pub header: FitsHeader,
    pub image_data: Option<FitsImageData>,
    pub index: usize,                   // 0 = primary, 1+ = extensions
    pub name: String,                   // EXTNAME keyword, or "Primary"/"Extension N"
}
```

For multi-extension FITS (MEF) files, parse all HDUs. The UI provides an HDU selector dropdown in the controls bar to switch between extensions.

### Size Cap

Refuse to load files larger than 512 MB of raw data. For files that exceed this limit, show an error: `"FITS file data exceeds 512 MB. Use an external FITS viewer for very large images."`.

## Rendering Pipeline

### Overview

The rendering pipeline transforms floating-point pixel values into BGRA8 bytes for display on a Cairo surface.

```
Raw f64 pixels
    |
    v
Clip to [minCut, maxCut]           -- user-adjustable range
    |
    v
Apply stretch function              -- Linear, Log, Sqrt, Squared, Asinh
    |
    v
Map through colormap LUT            -- 256-entry RGB lookup table
    |
    v
BGRA8 byte buffer                   -- ready for Cairo ImageSurface
```

### Stretch Functions

All stretch functions map a normalized input `x` in `[0, 1]` to an output in `[0, 1]`:

| Stretch | Formula | Description |
|---------|---------|-------------|
| Linear | `y = x` | Direct linear mapping |
| Log | `y = log10(1 + 9x) / log10(10)` | Logarithmic, enhances faint features |
| Sqrt | `y = sqrt(x)` | Square root |
| Squared | `y = x^2` | Emphasizes bright features |
| Asinh | `y = asinh(10x) / asinh(10)` | Hyperbolic arcsine, wide dynamic range |
| Histogram Eq | CDF-based equalization | Equalizes histogram distribution |

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stretch {
    Linear,
    Log,
    Sqrt,
    Squared,
    Asinh,
    HistogramEq,
}

fn apply_stretch(x: f64, stretch: Stretch, cdf: Option<&[f64]>) -> f64 {
    let x = x.clamp(0.0, 1.0);
    match stretch {
        Stretch::Linear => x,
        Stretch::Log => (1.0 + 9.0 * x).log10(),  // log10(10) = 1.0
        Stretch::Sqrt => x.sqrt(),
        Stretch::Squared => x * x,
        Stretch::Asinh => (10.0 * x).asinh() / (10.0_f64).asinh(),
        Stretch::HistogramEq => {
            if let Some(cdf) = cdf {
                let idx = (x * (cdf.len() - 1) as f64) as usize;
                cdf[idx.min(cdf.len() - 1)]
            } else {
                x
            }
        }
    }
}
```

### Colormaps

Each colormap is a 256-entry lookup table of (R, G, B) tuples.

| Colormap | Description |
|----------|-------------|
| Grayscale | Linear black to white |
| Inverted | Linear white to black |
| Heat | Black -> Red -> Yellow -> White |
| Cool | Black -> Blue -> Cyan -> White |
| Viridis | Perceptually uniform, blue -> green -> yellow |

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMap {
    Grayscale,
    Inverted,
    Heat,
    Cool,
    Viridis,
}

type ColorLut = [(u8, u8, u8); 256];

fn build_lut(colormap: ColorMap) -> ColorLut {
    let mut lut = [(0u8, 0u8, 0u8); 256];
    for i in 0..256 {
        let t = i as f64 / 255.0;
        lut[i] = match colormap {
            ColorMap::Grayscale => {
                let v = (t * 255.0) as u8;
                (v, v, v)
            }
            ColorMap::Inverted => {
                let v = ((1.0 - t) * 255.0) as u8;
                (v, v, v)
            }
            ColorMap::Heat => {
                let r = (t * 3.0).min(1.0);
                let g = ((t - 0.33) * 3.0).clamp(0.0, 1.0);
                let b = ((t - 0.67) * 3.0).clamp(0.0, 1.0);
                ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
            }
            ColorMap::Cool => {
                let r = ((t - 0.67) * 3.0).clamp(0.0, 1.0);
                let g = ((t - 0.33) * 3.0).clamp(0.0, 1.0);
                let b = (t * 3.0).min(1.0);
                ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
            }
            ColorMap::Viridis => viridis_sample(t),
        };
    }
    lut
}
```

### Auto-Cut (Percentile-Based)

When a FITS image is first loaded, automatically determine the display range:

```rust
fn auto_cut(pixels: &[f64]) -> (f64, f64) {
    // Sample up to 100K pixels for performance
    let sample_size = pixels.len().min(100_000);
    let step = if pixels.len() > sample_size {
        pixels.len() / sample_size
    } else {
        1
    };
    
    let mut sample: Vec<f64> = pixels.iter()
        .step_by(step)
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    
    sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    
    if sample.is_empty() {
        return (0.0, 1.0);
    }
    
    // 0.5th and 99.5th percentiles
    let low_idx = (sample.len() as f64 * 0.005) as usize;
    let high_idx = (sample.len() as f64 * 0.995) as usize;
    
    let min_cut = sample[low_idx.min(sample.len() - 1)];
    let max_cut = sample[high_idx.min(sample.len() - 1)];
    
    (min_cut, max_cut)
}
```

The auto-cut values populate the min/max spin buttons in the controls. Users can override manually.

### Cancellable Rendering

For large images, rendering can be cancelled (e.g., when the user changes a control before rendering finishes):

```rust
fn render_to_bgra(
    data: &FitsImageData,
    stretch: Stretch,
    colormap: ColorMap,
    vmin: f64,
    vmax: f64,
    cancelled: &AtomicBool,
) -> Option<Vec<u8>> {
    let npixels = data.width * data.height;
    let lut = build_lut(colormap);
    let cdf = if stretch == Stretch::HistogramEq {
        Some(compute_cdf(&data.pixels, vmin, vmax, 65536))
    } else {
        None
    };
    
    let stride = cairo::Format::ARgb32
        .stride_for_width(data.width as u32)
        .unwrap_or(data.width as i32 * 4) as usize;
    let mut bgra = vec![0u8; stride * data.height];
    let range = vmax - vmin;
    
    for y in 0..data.height {
        // Check cancellation every 64 rows
        if y % 64 == 0 && cancelled.load(Ordering::Relaxed) {
            return None;
        }
        
        for x in 0..data.width {
            let val = data.pixels[y * data.width + x];
            let normalized = ((val - vmin) / range).clamp(0.0, 1.0);
            let stretched = apply_stretch(normalized, stretch, cdf.as_deref());
            let lut_idx = (stretched * 255.0) as usize;
            let (r, g, b) = lut[lut_idx.min(255)];
            
            let offset = y * stride + x * 4;
            bgra[offset] = b;          // B
            bgra[offset + 1] = g;      // G
            bgra[offset + 2] = r;      // R
            bgra[offset + 3] = 255;    // A
        }
    }
    
    Some(bgra)
}
```

## WCS (World Coordinate System)

### Parsing from Header

```rust
pub struct WcsInfo {
    pub crpix1: f64,    // Reference pixel X
    pub crpix2: f64,    // Reference pixel Y
    pub crval1: f64,    // Reference world coordinate RA (degrees)
    pub crval2: f64,    // Reference world coordinate Dec (degrees)
    pub cd1_1: f64,     // CD matrix element (degrees/pixel)
    pub cd1_2: f64,
    pub cd2_1: f64,
    pub cd2_2: f64,
    pub north_angle: f64,  // Angle from pixel-Y to celestial north (degrees)
}
```

Parsing priority:
1. **CD matrix** (preferred): Read `CD1_1`, `CD1_2`, `CD2_1`, `CD2_2` directly.
2. **CDELT + CROTA2 fallback**: If CD matrix not present:
   ```rust
   let cdelt1 = header.get_f64("CDELT1")?;
   let cdelt2 = header.get_f64("CDELT2")?;
   let crota2 = header.get_f64("CROTA2").unwrap_or(0.0);
   let cos_r = crota2.to_radians().cos();
   let sin_r = crota2.to_radians().sin();
   cd1_1 = cdelt1 * cos_r;
   cd1_2 = -cdelt2 * sin_r;  // Note: sign depends on convention
   cd2_1 = cdelt1 * sin_r;
   cd2_2 = cdelt2 * cos_r;
   ```

### North Angle Calculation

```rust
fn compute_north_angle(wcs: &WcsInfo) -> f64 {
    // North is the direction of increasing Dec
    // In pixel coordinates, this is the direction of the CD2 column vector
    let angle = wcs.cd2_1.atan2(wcs.cd2_2).to_degrees();
    angle
}
```

### Forward Transform: Pixel to World

```rust
impl WcsInfo {
    /// Convert pixel coordinates (0-indexed) to sky coordinates (RA, Dec in degrees)
    pub fn pixel_to_world(&self, px: f64, py: f64) -> (f64, f64) {
        let dx = px - self.crpix1;
        let dy = py - self.crpix2;
        let ra = self.crval1 + self.cd1_1 * dx + self.cd1_2 * dy;
        let dec = self.crval2 + self.cd2_1 * dx + self.cd2_2 * dy;
        (ra, dec)
    }
}
```

This is the existing implementation. It uses a simple linear (tangent-plane) approximation, which is accurate for small fields of view. For large fields or near the poles, a full gnomonic (TAN) projection would be needed.

### Inverse Transform: World to Pixel

```rust
impl WcsInfo {
    /// Convert sky coordinates (RA, Dec in degrees) to pixel coordinates.
    /// Returns None if the CD matrix is singular.
    pub fn world_to_pixel(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        let dra = ra - self.crval1;
        let ddec = dec - self.crval2;
        
        let det = self.cd1_1 * self.cd2_2 - self.cd1_2 * self.cd2_1;
        if det.abs() < 1e-15 {
            return None;  // Singular matrix
        }
        
        let dx = (self.cd2_2 * dra - self.cd1_2 * ddec) / det;
        let dy = (-self.cd2_1 * dra + self.cd1_1 * ddec) / det;
        
        Some((dx + self.crpix1, dy + self.crpix2))
    }
}
```

### Coordinate Formatting

```rust
impl WcsInfo {
    /// Format RA in degrees to sexagesimal: HH:MM:SS.ss
    pub fn format_ra(ra_deg: f64) -> String {
        let ra_h = ra_deg / 15.0;
        let h = ra_h.floor() as i32;
        let m = ((ra_h - h as f64) * 60.0).floor() as i32;
        let s = ((ra_h - h as f64) * 3600.0 - m as f64 * 60.0).abs();
        format!("{:02}h{:02}m{:05.2}s", h, m, s)
    }
    
    /// Format Dec in degrees to sexagesimal: +/-DD MM'SS.s"
    pub fn format_dec(dec_deg: f64) -> String {
        let sign = if dec_deg < 0.0 { "-" } else { "+" };
        let abs = dec_deg.abs();
        let d = abs.floor() as i32;
        let m = ((abs - d as f64) * 60.0).floor() as i32;
        let s = ((abs - d as f64) * 3600.0 - m as f64 * 60.0).abs();
        format!("{}{}°{:02}'{:04.1}\"", sign, d, m, s)
    }
}
```

## Interactions

### Scroll Wheel Zoom

Existing implementation. Zoom toward the cursor position (or crosshair if locked).

```rust
scroll_controller.connect_scroll(move |controller, _dx, dy| {
    let mut t = transform.borrow_mut();
    let factor = if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
    
    // Get cursor position for zoom-toward-cursor
    let (cx, cy) = /* cursor position or center of canvas */;
    
    // Adjust offset so the point under cursor stays fixed
    let old_scale = t.scale;
    t.scale = (t.scale * factor).clamp(0.1, 50.0);
    let scale_ratio = t.scale / old_scale;
    t.offset_x = cx - (cx - t.offset_x) * scale_ratio;
    t.offset_y = cy - (cy - t.offset_y) * scale_ratio;
    
    drawing_area.queue_draw();
    Propagation::Stop
});
```

### Click-Drag Pan

Existing implementation. Left mouse button drag pans the image.

### Right-Click Crosshair

Right-click places a persistent crosshair at the clicked position (in image pixel coordinates).

```rust
let click = gtk::GestureClick::new();
click.set_button(3);  // Right mouse button
click.connect_pressed(move |_, _, x, y| {
    let t = transform.borrow();
    let img_x = (x - t.offset_x) / t.scale;
    let img_y = (y - t.offset_y) / t.scale;
    
    if img_x >= 0.0 && img_x < width as f64 && img_y >= 0.0 && img_y < height as f64 {
        *crosshair_pos.borrow_mut() = Some((img_x, img_y));
        *shared_cursor.borrow_mut() = Some((img_x, img_y));
        drawing_area.queue_draw();
    }
});
```

## Crosshair

### Rendering

The crosshair is drawn as two full-length lines (horizontal and vertical) in green with 70% opacity:

```rust
if let Some((cx, cy)) = *crosshair_pos.borrow() {
    let sx = cx * t.scale + t.offset_x;
    let sy = cy * t.scale + t.offset_y;
    
    cr.set_source_rgba(0.0, 1.0, 0.0, 0.7);
    cr.set_line_width(1.0);
    
    // Vertical line
    cr.move_to(sx, 0.0);
    cr.line_to(sx, widget_h as f64);
    cr.stroke().ok();
    
    // Horizontal line
    cr.move_to(0.0, sy);
    cr.line_to(widget_w as f64, sy);
    cr.stroke().ok();
    
    // Coordinate label near crosshair
    if let Some(ref wcs) = wcs {
        let (ra, dec) = wcs.pixel_to_world(cx, cy);
        let label = format!("({:.0}, {:.0}) RA: {} Dec: {}",
            cx, cy,
            WcsInfo::format_ra(ra),
            WcsInfo::format_dec(dec));
        // Draw label at (sx + 10, sy - 10) with semi-transparent background
    }
}
```

### Crosshair Label

- Position: 10px right and 10px above the crosshair intersection.
- Background: semi-transparent black rectangle behind text.
- Content: `"(px_x, px_y) RA: HH:MM:SS.ss Dec: +DD°MM'SS.s""`
- Also shows pixel value at the crosshair position.
- Survives zoom, pan, and rotation (re-rendered in draw function from image-space coords).

### Linked Crosshairs

When a crosshair is placed on the active tab, its sky position (RA, Dec) is computed via WCS and shared with all other tabs:

```rust
fn update_linked_crosshairs(&self) {
    let active_tab = self.get_active_tab();
    if let Some((px, py)) = active_tab.crosshair_pos() {
        if let Some(ref wcs) = active_tab.wcs {
            let (ra, dec) = wcs.pixel_to_world(px, py);
            
            // For each other tab, compute pixel position from world coords
            for tab in self.tabs.borrow().iter() {
                if tab.is_active() { continue; }
                if let Some(ref other_wcs) = tab.wcs {
                    if let Some((other_px, other_py)) = other_wcs.world_to_pixel(ra, dec) {
                        tab.set_linked_crosshair(Some((other_px, other_py)));
                    }
                }
            }
        }
    }
}
```

Linked crosshairs are drawn in a different color (yellow, 50% opacity) to distinguish from the primary crosshair.

## Sync Zoom

When sync zoom is active, changing zoom on the active tab adjusts all other tabs to show the same angular extent:

```rust
fn sync_zoom_to_other_tabs(&self, active_tab: &FitsTab) {
    let active_scale = active_tab.scale();
    let active_pixel_scale = active_tab.wcs.as_ref()
        .map(|wcs| (wcs.cd1_1.powi(2) + wcs.cd2_1.powi(2)).sqrt())  // deg/pixel
        .unwrap_or(1.0);
    
    for tab in self.tabs.borrow().iter() {
        if tab.is_active() { continue; }
        let other_pixel_scale = tab.wcs.as_ref()
            .map(|wcs| (wcs.cd1_1.powi(2) + wcs.cd2_1.powi(2)).sqrt())
            .unwrap_or(1.0);
        
        // Match angular extent: active_scale * active_pixel_scale = other_scale * other_pixel_scale
        let other_scale = active_scale * active_pixel_scale / other_pixel_scale;
        tab.set_scale(other_scale);
    }
}
```

## Blink Comparison

Toggle between the first two tabs at a configurable interval (default 500ms).

```rust
fn start_blink(&self) {
    let notebook = self.notebook.clone();
    let blink_active = self.blink_active.clone();
    
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if !*blink_active.borrow() {
            return ControlFlow::Break;
        }
        if notebook.n_pages() < 2 {
            return ControlFlow::Break;
        }
        let current = notebook.current_page().unwrap_or(0);
        let next = if current == 0 { 1 } else { 0 };
        notebook.set_current_page(Some(next));
        ControlFlow::Continue
    });
}
```

Future enhancement: fade transition using alpha blending between two rendered images on a single canvas.

### BlinkSession Model

```rust
pub struct BlinkSession {
    pub tab_indices: (usize, usize),    // Which two tabs to blink between
    pub interval_ms: u32,               // Blink interval in milliseconds
    pub active: bool,
}
```

## North-Up Rotation

Rotate the image so celestial north points up on screen.

```rust
fn apply_north_up(&self) {
    if let Some(ref wcs) = self.wcs {
        let north_angle = compute_north_angle(wcs);
        let mut t = self.transform.borrow_mut();
        t.rotation = -north_angle.to_radians();  // Rotate canvas by negative north angle
        
        // Check for parity flip (mirrored images)
        let det = wcs.cd1_1 * wcs.cd2_2 - wcs.cd1_2 * wcs.cd2_1;
        if det > 0.0 {
            t.flip_x = true;  // Mirror horizontally to correct parity
        }
        
        self.drawing_area.queue_draw();
    }
}
```

The `ViewTransform` struct is extended with `rotation` and `flip_x` fields:

```rust
struct ViewTransform {
    scale: f64,
    offset_x: f64,
    offset_y: f64,
    rotation: f64,      // Radians, applied around image center
    flip_x: bool,       // Horizontal flip for parity correction
}
```

Cairo transform application order in the draw function:
```rust
cr.translate(canvas_center_x, canvas_center_y);
cr.rotate(t.rotation);
if t.flip_x {
    cr.scale(-1.0, 1.0);
}
cr.scale(t.scale, t.scale);
cr.translate(-image_center_x, -image_center_y);
cr.translate(t.offset_x / t.scale, t.offset_y / t.scale);
```

## Header Panel

A collapsible side panel (or bottom panel) showing all FITS header cards with search/filter.

```
+--------------------------------------------+
| [Search: ___________]                       |
+--------------------------------------------+
| SIMPLE   = T           / Standard FITS     |
| BITPIX   = -32         / IEEE float 32-bit |
| NAXIS    = 2           / Number of axes    |
| NAXIS1   = 2048        / Width             |
| NAXIS2   = 2048        / Height            |
| OBJECT   = M31         / Target name       |
| TELESCOP = JWST        / Telescope         |
| ...                                        |
+--------------------------------------------+
```

### Implementation

```rust
pub struct HeaderPanel {
    widget: gtk::Box,
    search_entry: gtk::SearchEntry,
    list_box: gtk::ListBox,
    cards: Vec<FitsCard>,
}
```

Each header card row shows three columns:
- **Keyword** (8 chars, monospace, bold): `gtk::Label`
- **Value** (variable width): `gtk::Label` (selectable)
- **Comment** (dim): `gtk::Label` with `dim-label` CSS class

Filter: On `SearchEntry::connect_search_changed`, filter rows by case-insensitive substring match on keyword, value, or comment.

The header panel is placed in a `gtk::Paned` alongside the canvas, so users can resize it.

## Saved Coordinates

Bookmark RA/Dec positions for quick reference.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCoordinate {
    pub ra: f64,                    // RA in degrees
    pub dec: f64,                   // Dec in degrees
    pub label: String,              // User-assigned name, e.g., "M31 nucleus"
    pub saved_at: String,           // ISO 8601 timestamp
}
```

Storage: `{ProjectDirs::data_dir()}/saved_coordinates.json`. Maximum 50 entries.

### UI

- **Save button** in crosshair context (right-click menu or toolbar): Save the current crosshair position with a user-entered label.
- **Saved Coords popover**: Triggered from toolbar button. Shows list of saved coordinates as `adw::ActionRow` items with:
  - Title: label
  - Subtitle: `"RA: HH:MM:SS.ss  Dec: +DD°MM'SS.s""`
  - Suffix: delete button (flat, `user-trash-symbolic`)
  - Click action: place crosshair at saved position (compute pixel coords via WCS inverse transform)

### Coordinate Actions

- **Copy RA/Dec**: Copy formatted coordinates to clipboard. Context menu option on crosshair.
- **Search at Position**: Send RA/Dec to the Search module. Fire a callback that the main window wires to set the Search page's resolved coordinates and switch to the Search tab.

## ViewportMath

Utility functions for coordinate transforms between image space, canvas space, and widget space.

```rust
pub struct ViewportMath;

impl ViewportMath {
    /// Image pixel coords -> widget coords (screen position)
    pub fn image_to_widget(
        img_x: f64, img_y: f64,
        transform: &ViewTransform,
        canvas_w: f64, canvas_h: f64,
        img_w: f64, img_h: f64,
    ) -> (f64, f64) {
        // Apply rotation around image center
        let cx = img_x - img_w / 2.0;
        let cy = img_y - img_h / 2.0;
        let cos_r = transform.rotation.cos();
        let sin_r = transform.rotation.sin();
        let rx = cx * cos_r - cy * sin_r;
        let ry = cx * sin_r + cy * cos_r;
        let flip = if transform.flip_x { -1.0 } else { 1.0 };
        
        let wx = (rx * flip) * transform.scale + transform.offset_x + canvas_w / 2.0;
        let wy = ry * transform.scale + transform.offset_y + canvas_h / 2.0;
        (wx, wy)
    }
    
    /// Widget coords -> image pixel coords (for mouse interaction)
    pub fn widget_to_image(
        wx: f64, wy: f64,
        transform: &ViewTransform,
        canvas_w: f64, canvas_h: f64,
        img_w: f64, img_h: f64,
    ) -> (f64, f64) {
        // Inverse of image_to_widget
        let flip = if transform.flip_x { -1.0 } else { 1.0 };
        let rx = ((wx - canvas_w / 2.0 - transform.offset_x) / transform.scale) * flip;
        let ry = (wy - canvas_h / 2.0 - transform.offset_y) / transform.scale;
        let cos_r = transform.rotation.cos();
        let sin_r = transform.rotation.sin();
        // Inverse rotation
        let cx = rx * cos_r + ry * sin_r;
        let cy = -rx * sin_r + ry * cos_r;
        let img_x = cx + img_w / 2.0;
        let img_y = cy + img_h / 2.0;
        (img_x, img_y)
    }
}
```

## Controls Bar (FitsControls)

Extended from existing implementation at `src/ui/fits_controls.rs`:

```
[Stretch: v Linear] [Color: v Grayscale] [Min: [___]] [Max: [___]] [Auto] [HDU: v Primary]
```

| Control | Widget | Purpose |
|---------|--------|---------|
| Stretch | `gtk::DropDown` | Select stretch function |
| Color | `gtk::DropDown` | Select colormap |
| Min | `gtk::SpinButton` | Lower cut value |
| Max | `gtk::SpinButton` | Upper cut value |
| Auto | `gtk::Button` | Reset to auto-cut percentiles |
| HDU | `gtk::DropDown` | Select HDU for multi-extension FITS (hidden for single-HDU) |

## Models Summary

| Model | File | Description |
|-------|------|-------------|
| `FitsImageData` | `src/models/fits_image.rs` | Pixel data, dimensions, header, WCS, min/max |
| `FitsHdu` | `src/models/fits_image.rs` | Header + optional image data per HDU |
| `FitsHeader` | `src/models/fits_image.rs` (new) | Ordered list of FitsCard |
| `FitsCard` | `src/models/fits_image.rs` (new) | Keyword, value, comment, raw 80-char |
| `WcsInfo` | `src/models/fits_image.rs` | WCS parameters + transforms |
| `WorldCoordinate` | `src/models/fits_image.rs` (new) | `{ ra: f64, dec: f64 }` |
| `BlinkSession` | `src/ui/fits_viewer.rs` | Blink config |
| `SavedCoordinate` | `src/models/fits_image.rs` (new) | Bookmarked sky positions |
| `ViewTransform` | `src/ui/fits_canvas.rs` | Scale, offset, rotation, flip |

## Module Files

| File | Status | Changes |
|------|--------|---------|
| `src/models/fits_image.rs` | Existing | Add `FitsFile`, `FitsHdu`, `FitsHeader`, `FitsCard`, `WorldCoordinate`, `SavedCoordinate`; extend `WcsInfo` with `north_angle`, `world_to_pixel`, `format_ra`, `format_dec` |
| `src/helpers/fits_loader.rs` | Existing | Add pure Rust parser (`load_fits_pure`), keep cfitsio behind feature flag |
| `src/helpers/fits_renderer.rs` | Existing | Add `Squared`, `Asinh` stretches; add `Inverted`, `Cool` colormaps; add LUT-based rendering; add cancellation support |
| `src/ui/fits_viewer.rs` | Existing | Add sync zoom, north-up, saved coords toolbar buttons and logic |
| `src/ui/fits_tab.rs` | Existing | Add header panel, HDU selector, rotation/flip support |
| `src/ui/fits_canvas.rs` | Existing | Extend `ViewTransform` with rotation/flip; add right-click crosshair; add `ViewportMath` |
| `src/ui/fits_controls.rs` | Existing | Add Auto button, HDU dropdown, additional stretch/colormap options |
| `src/services/coordinate_store.rs` | New | Saved coordinates JSON persistence |

## GTK4/Adwaita Widget Mapping

| Concept | Widget |
|---------|--------|
| Tab container | `gtk::Notebook` |
| Image canvas | `gtk::DrawingArea` with Cairo |
| Controls bar | `gtk::Box` horizontal with `gtk::DropDown`, `gtk::SpinButton` |
| Header panel | `gtk::Paned` split, `gtk::ListBox` with search filter |
| Header search | `gtk::SearchEntry` |
| Saved coords popover | `gtk::Popover` with `gtk::ListBox` |
| Coordinate label | `gtk::Label` below canvas |
| Blink toggle | `gtk::ToggleButton` |
| File dialog | `gtk::FileDialog` with FITS filter |

## Error Handling

- **Invalid FITS file**: Show toast `"Not a valid FITS file: {reason}"`. Do not open a tab.
- **Unsupported BITPIX**: Show toast with the unsupported value. Do not crash.
- **File too large**: Show toast `"FITS file exceeds 512 MB size limit"`.
- **No image data in HDU**: Show message in the tab: `"This HDU contains no image data (table or empty)"`.
- **WCS not available**: Disable coordinate readout, linked crosshairs, north-up, sync zoom. Show `"No WCS"` in the coordinate label area.
- **Singular WCS matrix**: `world_to_pixel` returns `None`. Linked crosshairs are not shown for that tab.
- **NaN/Inf pixels**: Filter out during auto-cut. Render as black (or configurable background color).
