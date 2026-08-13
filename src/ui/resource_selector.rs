use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Build the list of power-of-two values within the inclusive `[min, max]`
/// range. Faithful port of the RAM scale construction in
/// `ResourceSelectorPanel.Configure` (Windows reference): walk `1, 2, 4, …`
/// up to `max`, keeping every value `>= min`.
fn build_pow2_values(min: u32, max: u32) -> Vec<u32> {
    let mut powers = Vec::new();
    let mut v: u32 = 1;
    while v <= max {
        if v >= min {
            powers.push(v);
        }
        // Guard against overflow before doubling past u32::MAX.
        if v > u32::MAX / 2 {
            break;
        }
        v *= 2;
    }
    powers
}

/// Index of the power-of-two value in `powers` nearest to `value`. Port of
/// `FindNearestPow2Index`: strict `<` comparison so ties resolve to the
/// smaller (earlier) power. Returns `0` for an empty slice.
fn find_nearest_pow2_index(powers: &[u32], value: u32) -> usize {
    let mut best_idx = 0usize;
    let mut best_diff = u32::MAX;
    for (i, &p) in powers.iter().enumerate() {
        let diff = p.abs_diff(value);
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
/// Snap `value` to the nearest power-of-two present in `powers`. Convenience
/// wrapper over [`find_nearest_pow2_index`]; returns `value` unchanged when
/// `powers` is empty.
fn find_nearest_pow2(powers: &[u32], value: u32) -> u32 {
    if powers.is_empty() {
        value
    } else {
        powers[find_nearest_pow2_index(powers, value)]
    }
}

/// A small circular "?" help button whose popover (and tooltip) explains the
/// adjacent field. Mirrors the per-field `TeachingTip`/`OnHelpClick` behaviour
/// of the Windows `ResourceSelectorPanel`.
fn build_help_button(title: &str, body: &str) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_size_request(240, -1);

    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("heading");
    title_label.set_halign(gtk::Align::Start);

    let body_label = gtk::Label::new(Some(body));
    body_label.set_halign(gtk::Align::Start);
    body_label.set_xalign(0.0);
    body_label.set_wrap(true);

    content.append(&title_label);
    content.append(&body_label);
    popover.set_child(Some(&content));

    let button = gtk::MenuButton::new();
    button.set_icon_name("help-about-symbolic");
    button.set_valign(gtk::Align::Center);
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(body));
    button.set_popover(Some(&popover));
    button
}

/// Configure a slider so it snaps to whole integers while dragging.
fn init_integer_scale(scale: &gtk::Scale) {
    scale.set_digits(0);
    scale.set_round_digits(0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.set_width_request(160);
}

pub struct ResourceSelector {
    pub container: gtk::Box,
    cores_spin: gtk::SpinButton,
    ram_spin: gtk::SpinButton,
    ram_scale: gtk::Scale,
    gpu_spin: gtk::SpinButton,
    /// Powers-of-two the RAM slider indexes into (slider value == index).
    ram_powers: Rc<RefCell<Vec<u32>>>,
    /// Re-entrancy guard so RAM box<->slider mirroring does not recurse.
    syncing: Rc<Cell<bool>>,
}

impl ResourceSelector {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);

        let group = adw::PreferencesGroup::builder()
            .title(crate::tr_en!("Resources (Fixed)"))
            .build();

        // ---- CPU cores: slider + spin share one adjustment (auto-sync) ----
        let cores_adj = gtk::Adjustment::new(2.0, 1.0, 16.0, 1.0, 4.0, 0.0);
        let cores_spin = gtk::SpinButton::new(Some(&cores_adj), 1.0, 0);
        cores_spin.set_valign(gtk::Align::Center);
        let cores_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&cores_adj));
        init_integer_scale(&cores_scale);

        let cores_row = adw::ActionRow::builder()
            .title(crate::tr_en!("CPU Cores"))
            .subtitle(crate::tr_en!("Number of CPU cores"))
            .build();
        cores_row.add_prefix(&build_help_button(
            crate::tr_en!("CPU Cores"),
            crate::tr_en!("Number of cores used by the session."),
        ));
        cores_row.add_suffix(&cores_scale);
        cores_row.add_suffix(&cores_spin);
        group.add(&cores_row);

        // ---- RAM: slider indexes powers-of-two, spin shows GB (manual sync) --
        let default_powers: Vec<u32> = vec![1, 2, 4, 8, 16, 32, 64, 128, 256];
        let ram_adj = gtk::Adjustment::new(8.0, 1.0, 256.0, 1.0, 8.0, 0.0);
        let ram_spin = gtk::SpinButton::new(Some(&ram_adj), 1.0, 0);
        ram_spin.set_valign(gtk::Align::Center);
        let ram_scale = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            0.0,
            (default_powers.len().saturating_sub(1)) as f64,
            1.0,
        );
        init_integer_scale(&ram_scale);
        ram_scale.set_value(find_nearest_pow2_index(&default_powers, 8) as f64);

        let ram_row = adw::ActionRow::builder()
            .title(crate::tr_en!("RAM (GB)"))
            .subtitle(crate::tr_en!("Memory allocation in gigabytes"))
            .build();
        ram_row.add_prefix(&build_help_button(
            crate::tr_en!("RAM (GB)"),
            crate::tr_en!(
                "System memory (RAM) to be used for the session. Slider snaps to powers of 2."
            ),
        ));
        ram_row.add_suffix(&ram_scale);
        ram_row.add_suffix(&ram_spin);
        group.add(&ram_row);

        let ram_powers = Rc::new(RefCell::new(default_powers));
        let syncing = Rc::new(Cell::new(false));

        // Spin -> slider: snap slider to the nearest power-of-two index.
        {
            let ram_scale = ram_scale.clone();
            let ram_powers = ram_powers.clone();
            let syncing = syncing.clone();
            ram_spin.connect_value_changed(move |spin| {
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                let idx = find_nearest_pow2_index(&ram_powers.borrow(), spin.value() as u32);
                ram_scale.set_value(idx as f64);
                syncing.set(false);
            });
        }
        // Slider -> spin: mirror the actual power-of-two value into the box.
        {
            let ram_spin = ram_spin.clone();
            let ram_powers = ram_powers.clone();
            let syncing = syncing.clone();
            ram_scale.connect_value_changed(move |scale| {
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                let powers = ram_powers.borrow();
                let idx = scale.value().round() as usize;
                if let Some(&gb) = powers.get(idx) {
                    ram_spin.set_value(gb as f64);
                }
                syncing.set(false);
            });
        }

        // ---- GPUs: slider + spin share one adjustment; 0 always allowed ----
        let gpu_adj = gtk::Adjustment::new(0.0, 0.0, 4.0, 1.0, 1.0, 0.0);
        let gpu_spin = gtk::SpinButton::new(Some(&gpu_adj), 1.0, 0);
        gpu_spin.set_valign(gtk::Align::Center);
        let gpu_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&gpu_adj));
        init_integer_scale(&gpu_scale);

        let gpu_row = adw::ActionRow::builder()
            .title(crate::tr_en!("GPUs"))
            .subtitle(crate::tr_en!("Number of GPU cores"))
            .build();
        gpu_row.add_prefix(&build_help_button(
            crate::tr_en!("GPUs"),
            crate::tr_en!("Number of GPUs to allocate for the session."),
        ));
        gpu_row.add_suffix(&gpu_scale);
        gpu_row.add_suffix(&gpu_spin);
        group.add(&gpu_row);

        container.append(&group);

        ResourceSelector {
            container,
            cores_spin,
            ram_spin,
            ram_scale,
            gpu_spin,
            ram_powers,
            syncing,
        }
    }

    pub fn set_core_options(&self, options: &[u32], default: u32) {
        // Slider shares the spin's adjustment, so setting the range/value on the
        // spin updates both widgets in lock-step.
        if let (Some(&min), Some(&max)) = (options.iter().min(), options.iter().max()) {
            self.cores_spin.set_range(min as f64, max as f64);
        }
        self.cores_spin.set_value(default as f64);
    }

    pub fn set_memory_options(&self, options: &[u32], default: u32) {
        let (min, max) = match (options.iter().min(), options.iter().max()) {
            (Some(&mn), Some(&mx)) => (mn, mx),
            _ => (1, default.max(1)),
        };

        // Build the power-of-two scale within the available range, matching the
        // Windows Configure(); fall back to the raw options (then the default)
        // if no power of two fits.
        let mut powers = build_pow2_values(min, max);
        if powers.is_empty() {
            powers = options.to_vec();
        }
        if powers.is_empty() {
            powers = vec![default.max(1)];
        }

        self.syncing.set(true);
        self.ram_spin.set_range(min as f64, max as f64);
        self.ram_scale
            .set_range(0.0, powers.len().saturating_sub(1) as f64);
        let idx = find_nearest_pow2_index(&powers, default);
        *self.ram_powers.borrow_mut() = powers;
        self.ram_scale.set_value(idx as f64);
        self.ram_spin.set_value(default as f64);
        self.syncing.set(false);
    }

    pub fn set_gpu_options(&self, options: &[u32]) {
        // GPUs always allow 0 even when the context minimum is higher.
        let max = options.iter().max().copied().unwrap_or(0);
        self.gpu_spin.set_range(0.0, max as f64);
        self.gpu_spin.set_value(0.0);
    }

    pub fn cores(&self) -> u32 {
        self.cores_spin.value() as u32
    }

    pub fn ram(&self) -> u32 {
        self.ram_spin.value() as u32
    }

    pub fn gpus(&self) -> u32 {
        self.gpu_spin.value() as u32
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_pow2_values_within_range() {
        assert_eq!(
            build_pow2_values(1, 256),
            vec![1, 2, 4, 8, 16, 32, 64, 128, 256]
        );
        assert_eq!(build_pow2_values(2, 16), vec![2, 4, 8, 16]);
        // Only powers of two that fall inside [min, max] survive.
        assert_eq!(build_pow2_values(3, 10), vec![4, 8]);
        assert_eq!(build_pow2_values(1, 1), vec![1]);
    }

    #[test]
    fn find_nearest_pow2_snaps_to_powers() {
        let powers = [1u32, 2, 4, 8, 16, 32, 64, 128, 256];
        // Exact hits.
        assert_eq!(find_nearest_pow2(&powers, 8), 8);
        assert_eq!(find_nearest_pow2(&powers, 256), 256);
        // Ties resolve to the smaller power (strict `<`, like the C# port).
        assert_eq!(find_nearest_pow2(&powers, 6), 4);
        // Nearest for non-powers.
        assert_eq!(find_nearest_pow2(&powers, 100), 128);
        assert_eq!(find_nearest_pow2(&powers, 300), 256);
        assert_eq!(find_nearest_pow2(&powers, 0), 1);
        // Index form agrees.
        assert_eq!(find_nearest_pow2_index(&powers, 100), 7);
        // Empty slice is a safe no-op.
        assert_eq!(find_nearest_pow2(&[], 42), 42);
        assert_eq!(find_nearest_pow2_index(&[], 42), 0);
    }
}
