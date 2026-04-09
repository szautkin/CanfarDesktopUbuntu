use gtk4::prelude::*;
use gtk4::{self as gtk};

/// Show a download progress dialog.
pub async fn show_download_dialog(
    parent: &impl IsA<gtk::Widget>,
    filename: &str,
) -> Option<std::path::PathBuf> {
    let root = parent.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::builder()
        .title("Save File")
        .initial_name(filename)
        .build();

    match dialog.save_future(root.as_ref()).await {
        Ok(file) => file.path(),
        Err(_) => None,
    }
}
