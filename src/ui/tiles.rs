//! The tile grid, and the one width a tile is.
//!
//! Two pages show a grid of tiles — the home page's destinations and the AI
//! Guide's tool categories — and they built the same `FlowBox` twice with the
//! same four settings. They then disagreed about the one thing that decides how
//! the grid reads: the home tiles asked for a width and the guide tiles did
//! not, so on a quarter-screen window the home page laid out three columns and
//! the guide page laid out one, each tile a full-width band with a sentence in
//! it.
//!
//! A tile's width is not a preference. It is what decides whether a grid reads
//! as a grid.

use gtk4::prelude::*;
use gtk4::{self as gtk};

/// How wide a tile asks to be.
///
/// Chosen so a grid still has more than one column in the window the app opens
/// at — a page there is about 800 px wide, and two tiles plus their gap fit in
/// it with room to spare. `panel_width_probe` reports the columns each page
/// gets, so this is checkable rather than a matter of taste.
pub const TILE_WIDTH: i32 = 240;

/// The most columns a grid of tiles is allowed.
///
/// Beyond three, a tile stops reading as a destination and starts reading as a
/// table row.
pub const MAX_COLUMNS: u32 = 3;

/// An empty grid of tiles.
///
/// Selection is off: tiles are buttons, and a `FlowBox` selection would add a
/// second, conflicting notion of "chosen" on top of the button's own
/// activation.
///
/// Homogeneous, so a short label and a long one do not produce two tile sizes
/// in the same grid — and reflowing rather than fixed columns, because a `Grid`
/// could not rewrap: on a narrow window its tiles were clipped, and on a short
/// one its later rows were unreachable.
pub fn grid(spacing: i32) -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.set_row_spacing(spacing as u32);
    flow.set_column_spacing(spacing as u32);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(MAX_COLUMNS);
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow
}

/// Give a tile the shared width, and cap how wide its prose may push it.
///
/// The cap is the point. A wrapping label's natural width is its whole
/// sentence, and a homogeneous `FlowBox` sizes every child to the widest — so
/// one tile with a long summary and no cap is what turned the AI Guide's grid
/// into a single column. `max_width_chars` is what tells the label it may wrap
/// sooner.
pub fn size(tile: &impl IsA<gtk::Widget>, prose: &[&gtk::Label]) {
    tile.as_ref().set_size_request(TILE_WIDTH, -1);
    for label in prose {
        label.set_max_width_chars(TILE_WIDTH / 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two columns fit in the window the app opens at.
    ///
    /// The reported symptom: at a quarter of a 4K screen the guide's tiles were
    /// one per row. A page there has about 800 px, so a tile that cannot fit
    /// twice into that is a tile that has given up on being a grid.
    #[test]
    fn a_grid_still_has_columns_at_a_quarter_screen() {
        // The page width at the window the app opens at, measured on the
        // running app: 1200 logical less the shell's own 403.
        const PAGE: i32 = 797;
        const GAP: i32 = 16;
        let columns = (PAGE + GAP) / (TILE_WIDTH + GAP);
        assert!(
            columns >= 2,
            "a {TILE_WIDTH} px tile gives {columns} column(s) in a {PAGE} px \
             page — the grid is a list"
        );
    }

    /// Both tile grids are built here.
    ///
    /// They were built twice and drifted on the one setting that mattered.
    #[test]
    fn every_tile_grid_comes_from_this_module() {
        let mut own: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "tiles.rs" {
                continue;
            }
            let code = crate::testing::code(&source);
            // A homogeneous FlowBox is a grid of TILES — every child the same
            // size, because they are peers. A ragged one is a row of chips,
            // which is a different thing and not this module's business.
            if code.contains("FlowBox::new()") && code.contains("set_homogeneous(true)") {
                own.push(name.to_string());
            }
        }
        assert!(
            own.is_empty(),
            "these build their own tile grid instead of `tiles::grid`: {own:#?}"
        );
    }
}
