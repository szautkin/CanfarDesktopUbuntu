use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

pub struct ResourceSelector {
    pub container: gtk::Box,
    cores_spin: gtk::SpinButton,
    ram_spin: gtk::SpinButton,
    gpu_spin: gtk::SpinButton,
}

impl ResourceSelector {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);

        let group = adw::PreferencesGroup::builder()
            .title("Resources (Fixed)")
            .build();

        // CPU cores
        let cores_adj = gtk::Adjustment::new(2.0, 1.0, 16.0, 1.0, 4.0, 0.0);
        let cores_spin = gtk::SpinButton::new(Some(&cores_adj), 1.0, 0);

        let cores_row = adw::ActionRow::builder()
            .title("CPU Cores")
            .subtitle("Number of CPU cores")
            .build();
        cores_row.add_suffix(&cores_spin);
        group.add(&cores_row);

        // RAM
        let ram_adj = gtk::Adjustment::new(8.0, 1.0, 256.0, 1.0, 8.0, 0.0);
        let ram_spin = gtk::SpinButton::new(Some(&ram_adj), 1.0, 0);

        let ram_row = adw::ActionRow::builder()
            .title("RAM (GB)")
            .subtitle("Memory allocation in gigabytes")
            .build();
        ram_row.add_suffix(&ram_spin);
        group.add(&ram_row);

        // GPU
        let gpu_adj = gtk::Adjustment::new(0.0, 0.0, 4.0, 1.0, 1.0, 0.0);
        let gpu_spin = gtk::SpinButton::new(Some(&gpu_adj), 1.0, 0);

        let gpu_row = adw::ActionRow::builder()
            .title("GPUs")
            .subtitle("Number of GPU cores")
            .build();
        gpu_row.add_suffix(&gpu_spin);
        group.add(&gpu_row);

        container.append(&group);

        ResourceSelector {
            container,
            cores_spin,
            ram_spin,
            gpu_spin,
        }
    }

    pub fn set_core_options(&self, options: &[u32], default: u32) {
        if let (Some(&min), Some(&max)) = (options.first(), options.last()) {
            self.cores_spin.set_range(min as f64, max as f64);
        }
        self.cores_spin.set_value(default as f64);
    }

    pub fn set_memory_options(&self, options: &[u32], default: u32) {
        if let (Some(&min), Some(&max)) = (options.first(), options.last()) {
            self.ram_spin.set_range(min as f64, max as f64);
        }
        self.ram_spin.set_value(default as f64);
    }

    pub fn set_gpu_options(&self, options: &[u32]) {
        if let (Some(&min), Some(&max)) = (options.first(), options.last()) {
            self.gpu_spin.set_range(min as f64, max as f64);
        }
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
