# FITS Viewer — Implementation Plan

## Current State

Skeleton exists across 6 files:
- `src/ui/fits_viewer.rs` — Tab host with Open/Blink buttons, multi-tab via gtk::Notebook
- `src/ui/fits_tab.rs` — Per-tab wrapper (canvas + controls)
- `src/ui/fits_canvas.rs` — DrawingArea with basic zoom/pan/crosshair, RGBA→BGRA conversion
- `src/ui/fits_controls.rs` — Stretch/colormap dropdowns, min/max spin buttons
- `src/helpers/fits_renderer.rs` — Linear/Log/Sqrt/HistogramEq stretch, Grayscale/Heat/Viridis colormaps
- `src/helpers/fits_loader.rs` — cfitsio wrapper behind `#[cfg(feature = "fits")]`, error fallback without it
- `src/models/fits_image.rs` — FitsImageData with Vec<f64>, WcsInfo with forward transform only

**Key gaps:** No pure Rust FITS parser, missing stretch modes (Squared, Asinh), missing colormaps (Inverted, Cool), no auto-cut, no WCS inverse (WorldToPixel), no north angle, no header panel, no HDU selection, no saved coordinates, no sync zoom, canvas lacks rotation/flip, crosshair not persistent across zoom/pan.

---

## Step 1: Pure Rust FITS Parser

**File:** `src/helpers/fits_parser.rs` (NEW)
**Register:** `src/helpers/mod.rs`
**Dependencies:** None (stdlib only)

```rust
pub const BLOCK_SIZE: usize = 2880;
pub const CARD_SIZE: usize = 80;
pub const MAX_DATA_BYTES: usize = 512 * 1024 * 1024;

pub struct FitsFile { pub hdus: Vec<FitsHdu> }

pub struct FitsHdu {
    pub header: FitsHeader,
    pub image_data: Option<Vec<f32>>,  // f32 for performance
    pub width: usize,
    pub height: usize,
    pub index: usize,
    pub name: String,
}

pub struct FitsHeader { pub cards: Vec<FitsCard> }

pub struct FitsCard {
    pub keyword: String,
    pub value: String,
    pub comment: String,
}

impl FitsHeader {
    pub fn get(&self, keyword: &str) -> Option<&str>;
    pub fn get_f64(&self, keyword: &str) -> Option<f64>;
    pub fn get_i64(&self, keyword: &str) -> Option<i64>;
}

pub fn load_fits(path: &Path) -> Result<FitsFile, String>;
```

### Parsing algorithm:
1. Read entire file to `Vec<u8>`, check size ≤ 512MB
2. Loop parsing HDUs until EOF:
   a. Parse header: read 2880-byte blocks, extract 80-char cards until `END`
   b. Parse card: chars 0-7 = keyword (trimmed), chars 8-9 = `= ` for value cards
   c. Value parsing: quoted strings (single quotes), numbers, booleans (T/F)
   d. Calculate data size: `|BITPIX/8| × NAXIS1 × NAXIS2 × ... × NAXISn`
   e. Align to 2880 bytes: `((size + 2879) / 2880) * 2880`
   f. If NAXIS ≥ 2 and NAXIS1 > 0 and NAXIS2 > 0: parse image data
   g. Else: skip data bytes

### Image data unpacking (big-endian):
- BITPIX 8: `u8` → `f32`
- BITPIX 16: `i16::from_be_bytes` → `f32`
- BITPIX 32: `i32::from_be_bytes` → `f32`
- BITPIX -32: `f32::from_be_bytes`
- BITPIX -64: `f64::from_be_bytes` → `f32`

Apply scaling: `physical = BZERO + BSCALE × raw`
Y-flip: reverse row order (FITS bottom-to-top → display top-to-bottom)
Track min/max excluding NaN/Inf.

### Tests:
- Parse a minimal valid FITS header (hand-crafted bytes)
- Parse BITPIX values correctly
- BSCALE/BZERO application
- Multi-HDU parsing

---

## Step 2: Models Update

**File:** `src/models/fits_image.rs` (MODIFY)
**Dependencies:** Step 1

### Changes:
1. `FitsImageData.pixels`: `Vec<f64>` → `Vec<f32>`
2. `FitsImageData.min_val/max_val`: `f64` → `f32`
3. Add header cards storage alongside HashMap
4. Extend `WcsInfo`:

```rust
pub struct WcsInfo {
    pub crpix1: f64, pub crpix2: f64,
    pub crval1: f64, pub crval2: f64,
    pub cd1_1: f64, pub cd1_2: f64,
    pub cd2_1: f64, pub cd2_2: f64,
    pub ctype1: String, pub ctype2: String,
}

impl WcsInfo {
    pub fn pixel_to_sky(&self, px: f64, py: f64) -> (f64, f64);        // existing
    pub fn world_to_pixel(&self, ra: f64, dec: f64) -> Option<(f64, f64)>;  // NEW: invert CD matrix
    pub fn north_angle(&self) -> f64;      // NEW: atan2(-cd1_2, cd2_2) in degrees
    pub fn has_parity_flip(&self) -> bool;  // NEW: det(CD) > 0
    pub fn pixel_scale_arcsec(&self) -> f64; // NEW: geometric mean × 3600
    pub fn format_ra(ra_deg: f64) -> String;    // existing
    pub fn format_dec(dec_deg: f64) -> String;  // existing
    pub fn format_for_resolver(ra: f64, dec: f64) -> String; // NEW: CADC format
    pub fn from_header(header: &FitsHeader) -> Option<Self>;  // NEW: parse with CDELT+CROTA2 fallback
}
```

`world_to_pixel` inverse:
```
det = cd1_1*cd2_2 - cd1_2*cd2_1
if |det| < 1e-30: return None
dra = ra - crval1; ddec = dec - crval2
px = crpix1 + (cd2_2*dra - cd1_2*ddec) / det
py = crpix2 + (-cd2_1*dra + cd1_1*ddec) / det
```

Add new structs:
```rust
#[derive(Debug, Clone)]
pub struct WorldCoordinate { pub ra: f64, pub dec: f64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCoordinate { pub ra: f64, pub dec: f64, pub label: String, pub saved_at: String }

pub struct BlinkSession { pub tab_a: usize, pub tab_b: usize, pub interval_ms: u32, pub active: bool }
```

---

## Step 3: Renderer Update

**File:** `src/helpers/fits_renderer.rs` (MODIFY)
**Dependencies:** Step 2

### Changes:
1. Add to `Stretch` enum: `Squared`, `Asinh`
2. Add to `ColorMap` enum: `Inverted`, `Cool`
3. Change pixel type from `f64` to `f32` throughout
4. Add `auto_cut(pixels: &[f32]) -> (f32, f32)`:
   - Sample up to 100K pixels (stride = len/100000)
   - Sort sampled values
   - Return (sample[0.5%], sample[99.5%])
5. Add `render_to_bgra(...)` → `Vec<u8>` (BGRA byte order for Cairo, skip RGBA→BGRA conversion)
6. Pre-compute 256-entry color LUT before pixel loop
7. Stretch formulas:
   - Squared: `normalized²`
   - Asinh: `asinh(10×normalized) / asinh(10)`
8. Colormap LUTs:
   - Inverted: `(255-i, 255-i, 255-i)`
   - Cool: cyan→magenta interpolation

---

## Step 4: Canvas Rewrite

**File:** `src/ui/fits_canvas.rs` (MODIFY)
**Dependencies:** Steps 2, 3

### Changes:
1. Extend `ViewTransform`:
```rust
struct ViewTransform {
    scale: f64,
    offset_x: f64,
    offset_y: f64,
    rotation: f64,  // radians (NEW)
    flip_x: bool,   // parity flip (NEW)
}
```

2. Add `ViewportMath` functions:
```rust
fn image_to_widget(ix: f64, iy: f64, t: &ViewTransform, img_w: f64, img_h: f64, canvas_w: f64, canvas_h: f64) -> (f64, f64);
fn widget_to_image(wx: f64, wy: f64, t: &ViewTransform, img_w: f64, img_h: f64, canvas_w: f64, canvas_h: f64) -> (f64, f64);
```

3. Update `setup_draw`:
   - Accept BGRA buffer directly (no conversion)
   - Apply full transform: `cr.translate(center)` → `cr.rotate(rotation)` → `cr.scale(flip ? -scale : scale, scale)` → `cr.translate(-center)` → `cr.translate(offset)`
   - Draw persistent crosshair (green lines + coord label with background box)
   - Draw linked crosshair (yellow dashed lines)

4. Zoom-toward-cursor: Adjust offset so point under cursor stays fixed:
```rust
let ratio = new_scale / old_scale;
offset_x = cursor_x - (cursor_x - offset_x) * ratio;
offset_y = cursor_y - (cursor_y - offset_y) * ratio;
```

5. Right-click crosshair: `gtk::GestureClick` button 3 → store persistent crosshair position in image coords

6. Public methods:
```rust
pub fn set_linked_crosshair(&self, world: Option<WorldCoordinate>);
pub fn crosshair_image_pos(&self) -> Option<(f64, f64)>;
pub fn set_rotation(&self, radians: f64);
pub fn set_flip_x(&self, flip: bool);
```

---

## Step 5: Update fits_loader.rs

**File:** `src/helpers/fits_loader.rs` (MODIFY)
**Dependencies:** Steps 1, 2

Replace the `#[cfg(not(feature = "fits"))]` fallback with the pure parser:
```rust
#[cfg(not(feature = "fits"))]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    let fits_file = fits_parser::load_fits(path)?;
    // Find first HDU with image data
    let hdu = fits_file.hdus.iter().find(|h| h.image_data.is_some())
        .ok_or("No image HDU found")?;
    // Build FitsImageData from parser output
    // ...
}
```

Add:
```rust
pub fn load_fits_full(path: &Path) -> Result<FitsFile, String>;  // expose all HDUs
```

---

## Step 6: FitsTab + Header Panel

**File:** `src/ui/fits_tab.rs` (MODIFY)
**Dependencies:** Steps 1-5

### Changes:
1. Use `gtk::Paned` for horizontal split: canvas (left) + header panel (right, collapsible)
2. Store all HDUs: `hdus: Rc<RefCell<Vec<FitsHdu>>>`
3. Store current HDU index: `current_hdu: Rc<RefCell<usize>>`
4. Add `switch_hdu(index)` method
5. Expose WCS: `pub fn wcs(&self) -> Option<&WcsInfo>`
6. Expose crosshair: `pub fn crosshair_world(&self) -> Option<WorldCoordinate>`

### Header Panel (inside FitsTab):
```rust
struct HeaderPanel {
    widget: gtk::Box,
    search_entry: gtk::SearchEntry,
    list_box: gtk::ListBox,
    all_cards: Vec<FitsCard>,
}
```
- `gtk::SearchEntry` at top
- `gtk::ListBox` with rows: keyword (bold mono) | value (selectable) | comment (dim)
- Filter on `search_entry.connect_search_changed` — case-insensitive match on keyword/value/comment
- Toggle visibility via button in FitsControls

---

## Step 7: FitsControls Update

**File:** `src/ui/fits_controls.rs` (MODIFY)
**Dependencies:** Step 3

### Changes:
1. Stretch items: `["Linear", "Log", "Sqrt", "Squared", "Asinh", "Histogram Eq"]`
2. Colormap items: `["Grayscale", "Inverted", "Heat", "Cool", "Viridis"]`
3. Add "Auto" button → calls `auto_cut`, sets min/max spins
4. Add HDU dropdown (hidden for single-HDU): `gtk::DropDown` with HDU names
5. Add "Header" toggle button (show/hide header panel)
6. Update `stretch()` and `colormap()` for new variants

---

## Step 8: Tab Host Update (Linked Crosshairs, Sync Zoom, Blink)

**File:** `src/ui/fits_viewer.rs` (MODIFY)
**Dependencies:** Steps 2, 4, 6

### Toolbar additions:
- **Sync Zoom** toggle button (`zoom-fit-best-symbolic`)
- **North Up** button (`find-location-symbolic`)
- **Saved Coords** menu button (`starred-symbolic`) with popover

### Linked crosshairs:
```rust
// When active tab's crosshair position changes:
fn update_linked_crosshairs(&self) {
    let active = self.tabs[active_idx];
    let world_pos = active.crosshair_world()?;
    for (i, tab) in self.tabs.iter().enumerate() {
        if i != active_idx {
            tab.canvas.set_linked_crosshair(Some(world_pos.clone()));
        }
    }
}
```

### Sync zoom:
```rust
fn sync_zoom(&self) {
    let ref_tab = self.tabs[active_idx];
    let ref_scale = ref_tab.canvas.scale();
    let ref_pix_arcsec = ref_tab.wcs()?.pixel_scale_arcsec();
    let angular_zoom = ref_scale * ref_pix_arcsec;
    
    for (i, tab) in self.tabs.iter().enumerate() {
        if i != active_idx {
            if let Some(wcs) = tab.wcs() {
                let matched_scale = angular_zoom / wcs.pixel_scale_arcsec();
                tab.canvas.set_scale(matched_scale);
            }
        }
    }
}
```

### North Up:
```rust
fn set_north_up(&self) {
    let tab = self.tabs[active_idx];
    if let Some(wcs) = tab.wcs() {
        let angle_rad = -wcs.north_angle().to_radians();
        tab.canvas.set_rotation(angle_rad);
        tab.canvas.set_flip_x(wcs.has_parity_flip());
    }
}
```

### Blink update:
Current blink alternates notebook pages. Improve: overlay second image on canvas with opacity fade using `glib::timeout_add_local` at configurable interval.

---

## Step 9: Saved Coordinates Store

**File:** `src/services/coordinate_store.rs` (NEW)
**Register:** `src/services/mod.rs`
**Dependencies:** Step 2

```rust
pub struct CoordinateStore { file_path: PathBuf }

impl CoordinateStore {
    pub fn new() -> Self;
    pub fn load(&self) -> Vec<SavedCoordinate>;
    pub fn save(&self, coord: SavedCoordinate) -> Result<(), String>;
    pub fn remove(&self, index: usize) -> Result<(), String>;
}
```

Max 50 entries. Storage: `~/.local/share/net.canfar/Verbinal/saved_coordinates.json`.

Popover in toolbar: `gtk::Popover` with `gtk::ListBox` of saved coordinates. Each row: label + "RA Dec" subtitle. Click → navigate canvas to that position. "Save current" button at bottom. "Delete" button per row.

---

## Step 10: Integration

**Files to modify:**
- `src/ui/main_window.rs` — Wire "Open in FITS Viewer" events from Storage/Research:
  ```rust
  // When storage browser fires OpenInFitsViewerRequested:
  fits_viewer.load_from_path(&path);
  view_stack.set_visible_child_name("fits");
  ```
- `src/helpers/mod.rs` — Add `pub mod fits_parser;`
- `src/services/mod.rs` — Add `pub mod coordinate_store;`
- `src/helpers/fits_loader.rs` — Wire pure parser as default
- Remove `fitsio` from Cargo.toml default path (keep as optional feature only)

---

## Implementation Order

| Phase | Step | Effort | Description |
|-------|------|--------|-------------|
| 1 | Step 1 | 2 days | Pure Rust FITS parser (core algorithm, tests) |
| 1 | Step 2 | 1 day | Models update (f32, WcsInfo inverse, new structs) |
| 2 | Step 3 | 1 day | Renderer (new stretches, colormaps, BGRA, auto-cut) |
| 2 | Step 5 | 0.5 day | Wire pure parser into fits_loader |
| 3 | Step 4 | 2 days | Canvas rewrite (rotation, flip, viewport math, crosshair) |
| 3 | Step 7 | 0.5 day | Controls update (new dropdowns, Auto, HDU) |
| 4 | Step 6 | 1 day | FitsTab + header panel |
| 4 | Step 8 | 1.5 days | Tab host (linked crosshairs, sync zoom, north-up) |
| 5 | Step 9 | 0.5 day | Saved coordinates store |
| 5 | Step 10 | 0.5 day | Integration |
| **Total** | | **~10.5 days** | |

## Critical Path

```
Step 1 (parser) → Step 2 (models) → Step 3 (renderer) → Step 5 (loader)
                                   → Step 4 (canvas) → Step 6 (tab+header) → Step 8 (host)
Step 9 (coords store) can be done in parallel with Phase 3-4
```
