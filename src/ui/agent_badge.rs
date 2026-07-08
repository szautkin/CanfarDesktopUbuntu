//! Agent provenance badge — a small "wand" button that, on click, reveals a
//! popover attributing an entity to the AI agent/client that created it.
//!
//! Mirrors `Views/Controls/AgentBadge.xaml` from the Windows reference app:
//! a compact accent-tinted glyph button whose flyout lists the agent (tool),
//! client, timestamp and short fingerprint. Self-contained and synchronous.

use gtk4::prelude::*;
use gtk4::{self as gtk};

use crate::models::agent_attribution::AgentAttribution;

/// Build a badge widget for the given attribution.
///
/// Returns a horizontal `gtk::Box` wrapping a flat [`gtk::MenuButton`]. Clicking
/// the button pops up a popover with the attribution details. The wrapping box
/// keeps the badge vertically centred wherever it is embedded (e.g. inline in a
/// list-row title).
pub fn agent_badge(attr: &AgentAttribution) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    container.set_valign(gtk::Align::Center);

    let button = gtk::MenuButton::new();
    button.set_icon_name("applications-science-symbolic");
    button.set_valign(gtk::Align::Center);
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&format!("Created by {}", attr.client)));

    let popover = build_popover(attr);
    button.set_popover(Some(&popover));

    container.append(&button);
    container
}

/// Assemble the popover content that describes the attribution.
fn build_popover(attr: &AgentAttribution) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_position(gtk::PositionType::Top);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    // Match the XAML flyout's comfortable minimum width.
    content.set_size_request(300, -1);

    // Heading row: wand icon + "Created by AI agent".
    let heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    heading_row.set_valign(gtk::Align::Center);
    let heading_icon = gtk::Image::from_icon_name("applications-science-symbolic");
    heading_icon.add_css_class("accent");
    let heading_label = gtk::Label::new(Some("Created by AI agent"));
    heading_label.add_css_class("heading");
    heading_label.set_halign(gtk::Align::Start);
    heading_row.append(&heading_icon);
    heading_row.append(&heading_label);
    content.append(&heading_row);

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Key/value grid.
    let grid = gtk::Grid::new();
    grid.set_column_spacing(10);
    grid.set_row_spacing(4);

    add_row(&grid, 0, "Created by", &attr.client, false);
    add_row(&grid, 1, "Agent", &attr.tool, true);
    add_row(&grid, 2, "Applied", &attr.timestamp, false);
    add_row(&grid, 3, "Fingerprint", &attr.fingerprint, true);

    content.append(&grid);

    popover.set_child(Some(&content));
    popover
}

/// Append a labelled key/value pair to `grid` at `row`.
///
/// `mono` renders the value in a monospace face (for identifiers such as the
/// tool name and fingerprint), mirroring the `FontFamily="Consolas"` values in
/// the XAML.
fn add_row(grid: &gtk::Grid, row: i32, key: &str, value: &str, mono: bool) {
    let key_label = gtk::Label::new(Some(key));
    key_label.set_halign(gtk::Align::Start);
    key_label.set_valign(gtk::Align::Start);
    key_label.add_css_class("caption");
    key_label.add_css_class("dim-label");

    let value_label = gtk::Label::new(Some(value));
    value_label.set_halign(gtk::Align::Start);
    value_label.set_hexpand(true);
    value_label.set_xalign(0.0);
    value_label.set_wrap(true);
    value_label.set_selectable(true);
    value_label.add_css_class("caption");
    if mono {
        value_label.add_css_class("monospace");
    }

    grid.attach(&key_label, 0, row, 1, 1);
    grid.attach(&value_label, 1, row, 1, 1);
}
