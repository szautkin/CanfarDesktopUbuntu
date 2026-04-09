# Storage Module -- Implementation Plan

## Current State

The Storage module has a **working skeleton** with basic browse and navigate functionality:

- `src/models/vospace_node.rs` -- `VoSpaceNode` with `name`, `uri`, `node_type`, `size`, `date`, `content_type`. Has `is_container()` and `size_display()`. **Missing**: `is_public` field, `icon_name()` method by extension, `path()` helper.
- `src/helpers/vospace_parser.rs` -- Parses VOSpace XML, extracts child `<node>` elements with properties (`length`, `date`, `type`). Sorts folders-first alphabetically. **Missing**: `ispublic` property parsing, `detail=max` query parameter usage.
- `src/services/vospace_service.rs` -- Has `list_nodes()`, `create_folder()`, `delete_node()`, `download_url()`, `download_file()`. **Missing**: `upload_file()` method, `detail=max&limit=500` parameters on list.
- `src/ui/vospace_browser.rs` -- Has toolbar (Up, New Folder, Refresh), breadcrumb label (plain text, not clickable), file list (`ListBox` with `ActionRow`), status bar. Has `refresh()`, `go_up()`, `make_file_row()`. **Missing**: `on_row_activated()` is empty stub, `create_folder_dialog()` is empty stub, upload button/dialog, download wiring, delete confirmation, context menu, breadcrumb navigation, sorting, drag-and-drop, icon mapping by extension.

The service is registered in `AppServices` at `src/state.rs` line 18 as `pub vospace: VoSpaceService`.

---

## Step 1: Extend VoSpaceNode Model

**File**: `src/models/vospace_node.rs`

### 1a: Add `is_public` field

Add to the struct (after line 17, `content_type`):

```rust
pub is_public: bool,
```

Update the struct initialization in `vospace_parser.rs` (line 88) to include `is_public: false` (default, updated by parsing).

### 1b: Add `path()` helper

Extract the relative path from the VOSpace URI:

```rust
impl VoSpaceNode {
    /// Extract the relative path from the URI.
    /// E.g., "vos://cadc.nrc.ca~arc/home/user/data/file.fits" -> "data/file.fits"
    pub fn path(&self) -> String {
        // Find "/home/{username}/" and return everything after it
        if let Some(idx) = self.uri.find("/home/") {
            let after_home = &self.uri[idx + 6..]; // skip "/home/"
            // Skip the username segment
            if let Some(slash_idx) = after_home.find('/') {
                return after_home[slash_idx + 1..].to_string();
            }
        }
        self.name.clone()
    }
}
```

### 1c: Add `icon_name()` method

```rust
impl VoSpaceNode {
    pub fn icon_name(&self) -> &'static str {
        if self.is_container() {
            return "folder-symbolic";
        }
        let ext = self.name.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "fits" | "fit" | "fts" | "fz" => "image-x-generic-symbolic",
            "py" | "sh" | "bash" | "r" | "jl" | "rs" => "text-x-script-symbolic",
            "csv" | "tsv" | "dat" | "cat" | "vot" => "x-office-spreadsheet-symbolic",
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "svg" => "image-x-generic-symbolic",
            "pdf" | "doc" | "docx" | "odt" | "txt" | "md" | "tex" | "rtf" => "x-office-document-symbolic",
            "tar" | "gz" | "bz2" | "xz" | "zip" | "7z" | "rar" => "package-x-generic-symbolic",
            "ipynb" => "accessories-text-editor-symbolic",
            _ => "text-x-generic-symbolic",
        }
    }
}
```

### 1d: Add `date_display()` method

```rust
impl VoSpaceNode {
    pub fn date_display(&self) -> String {
        match &self.date {
            Some(d) => {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(d) {
                    dt.format("%Y-%m-%d").to_string()
                } else if d.len() > 10 {
                    d[..10].to_string()
                } else {
                    d.clone()
                }
            }
            None => String::new(),
        }
    }
}
```

**Dependencies**: None.

---

## Step 2: Complete VoSpaceParser

**File**: `src/helpers/vospace_parser.rs`

### 2a: Parse `ispublic` property

In `parse_single_node()` (around line 73), add parsing for the `ispublic` property:

```rust
let mut is_public: bool = false;

// Inside the property parsing loop (line 73-86):
} else if prop_uri.contains("ispublic") {
    is_public = text.eq_ignore_ascii_case("true");
}
```

Update the `Some(VoSpaceNode { ... })` return (line 88) to include `is_public`.

### 2b: Robustness for type attribute detection

The existing code at line 53-55 already handles both namespaced and plain `type` attributes. This is correct. No changes needed.

### 2c: Add date format normalization

Some CADC responses return dates in various formats. Normalize in the parser:

```rust
} else if prop_uri.contains("date") && !prop_uri.contains("groupread") {
    // Normalize date format
    let date_str = text.to_string();
    date = Some(date_str);
}
```

The existing code is adequate here. The `date_display()` method on the model handles formatting.

**Dependencies**: Step 1 (is_public field on model).

---

## Step 3: Complete VoSpaceService

**File**: `src/services/vospace_service.rs`

### 3a: Add `detail=max&limit=500` to list query

Update `list_nodes()` (line 25). The current URL construction at `self.endpoints.vospace_nodes_url(username, path)` needs the query params appended:

```rust
pub async fn list_nodes(
    &self,
    token: &str,
    username: &str,
    path: &str,
) -> Result<Vec<VoSpaceNode>, ApiError> {
    let base_url = self.endpoints.vospace_nodes_url(username, path);
    let url = format!("{}?detail=max&limit=500", base_url);
    // rest remains the same
}
```

### 3b: Add `upload_file()` method

After `download_file()` (line 107), add:

```rust
/// Upload a file to the user's VOSpace storage.
pub async fn upload_file(
    &self,
    token: &str,
    username: &str,
    remote_path: &str,
    local_path: &std::path::Path,
) -> Result<u64, ApiError> {
    let url = self.endpoints.vospace_files_url(username, remote_path);
    let file_bytes = tokio::fs::read(local_path)
        .await
        .map_err(|e| ApiError::Network(format!("Read error: {}", e)))?;
    let len = file_bytes.len() as u64;

    let resp = self.client.put(&url)
        .bearer_auth(token)
        .header("Content-Type", "application/octet-stream")
        .body(file_bytes)
        .send()
        .await?;

    check_response(resp).await?;
    Ok(len)
}
```

**Note**: For very large files, a streaming upload with `reqwest::Body::wrap_stream()` would be preferred. For the initial implementation, reading the whole file into memory is acceptable for files under ~200MB.

**Dependencies**: None.

---

## Step 4: Config Endpoints

**File**: `src/config.rs` (wherever `ApiEndpoints` is defined)

### 4a: Verify endpoint methods exist

Check that `vospace_nodes_url()` and `vospace_files_url()` are implemented. Based on `src/services/vospace_service.rs` usage, they already exist. Verify they construct URLs like:

- `{api_base_url}/arc/nodes/home/{username}/{path}` for node operations
- `{api_base_url}/arc/files/home/{username}/{path}` for file content

If `vospace_files_url()` is missing from the config, add it:

```rust
pub fn vospace_files_url(&self, username: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{}{}/{}", self.config.api_base_url, self.config.vospace_files_path, username)
    } else {
        format!("{}{}/{}/{}", self.config.api_base_url, self.config.vospace_files_path, username, path)
    }
}
```

**Dependencies**: None.

---

## Step 5: UI -- Breadcrumb Bar

**File**: `src/ui/vospace_browser.rs`

### 5a: Replace plain label with clickable breadcrumbs

Replace `breadcrumb_label: gtk::Label` (line 16) with a `gtk::Box`:

```rust
breadcrumb_box: gtk::Box,
```

In the constructor (line 33), replace the label with:

```rust
let breadcrumb_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
breadcrumb_box.set_hexpand(true);
breadcrumb_box.set_halign(gtk::Align::Start);
toolbar.append(&breadcrumb_box);
```

### 5b: Add `update_breadcrumbs()` method

```rust
fn update_breadcrumbs(&self) {
    // Clear existing breadcrumb buttons
    while let Some(child) = self.breadcrumb_box.first_child() {
        self.breadcrumb_box.remove(&child);
    }

    // Root button "/"
    let root_btn = gtk::Button::with_label("/");
    root_btn.add_css_class("flat");
    {
        let current_path = self.current_path.clone();
        let browser = /* weak ref or clone pattern */;
        root_btn.connect_clicked(move |_| {
            *current_path.borrow_mut() = String::new();
            // Trigger refresh
        });
    }
    self.breadcrumb_box.append(&root_btn);

    let path = self.current_path.borrow().clone();
    if path.is_empty() { return; }

    let segments: Vec<&str> = path.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let separator = gtk::Label::new(Some("/"));
        separator.add_css_class("dim-label");
        self.breadcrumb_box.append(&separator);

        if i == segments.len() - 1 {
            // Last segment: plain label (not clickable)
            let label = gtk::Label::new(Some(segment));
            label.add_css_class("heading");
            self.breadcrumb_box.append(&label);
        } else {
            // Clickable button
            let btn = gtk::Button::with_label(segment);
            btn.add_css_class("flat");
            let nav_path = segments[..=i].join("/");
            let current_path = self.current_path.clone();
            let file_list_box = self.file_list_box.clone(); // for refresh trigger
            btn.connect_clicked(move |_| {
                *current_path.borrow_mut() = nav_path.clone();
                // Trigger refresh
            });
            self.breadcrumb_box.append(&btn);
        }
    }
}
```

### 5c: Call in refresh()

In `refresh()` (line 172), after setting `self.breadcrumb_label.set_text(...)`, replace with:

```rust
self.update_breadcrumbs();
```

**Dependencies**: None. Can be done independently.

---

## Step 6: UI -- File List Improvements

**File**: `src/ui/vospace_browser.rs`

### 6a: Update `make_file_row()` to use `icon_name()`

Replace the icon logic in `make_file_row()` (lines 183-190) with:

```rust
let icon_name = node.icon_name();
```

### 6b: Add date column to row suffix

In `make_file_row()`, add a date label:

```rust
let date_str = node.date_display();
if !date_str.is_empty() {
    let date_label = gtk::Label::new(Some(&date_str));
    date_label.add_css_class("dim-label");
    date_label.add_css_class("caption");
    date_label.set_margin_end(8);
    row.add_suffix(&date_label);
}
```

### 6c: Store current node list for row lookup

Add a field to `VoSpaceBrowser`:

```rust
nodes: Rc<RefCell<Vec<VoSpaceNode>>>,
```

In `refresh()`, after populating the list, store the nodes:

```rust
*self.nodes.borrow_mut() = nodes.clone();
```

### 6d: Implement `on_row_activated()` for folder navigation

Replace the empty stub (line 236-238):

```rust
async fn on_row_activated(&self, idx: usize) {
    let nodes = self.nodes.borrow();
    let Some(node) = nodes.get(idx) else { return; };

    if node.is_container() {
        let current = self.current_path.borrow().clone();
        let new_path = if current.is_empty() {
            node.name.clone()
        } else {
            format!("{}/{}", current, node.name)
        };
        *self.current_path.borrow_mut() = new_path;
        drop(nodes); // Release borrow before async
        self.refresh().await;
    }
    // For files: no action on double-click (could show preview)
}
```

### 6e: Column sorting headers (optional enhancement)

Add sort state tracking and clickable column headers above the list. For MVP, the folders-first alphabetical sort from the parser is sufficient. Add sorting later by storing `sort_column` and `sort_direction` state and re-sorting the node list on click.

**Dependencies**: Steps 1 (icon_name, date_display), 2 (parser).

---

## Step 7: UI -- Toolbar Completion

**File**: `src/ui/vospace_browser.rs`

### 7a: Add Upload button

In the constructor, after `new_folder_btn` (line 44):

```rust
let upload_btn = gtk::Button::from_icon_name("document-send-symbolic");
upload_btn.set_tooltip_text(Some("Upload file"));
toolbar.append(&upload_btn);
```

Wire click handler:

```rust
let b = browser.clone();
upload_btn.connect_clicked(move |_| {
    let b = b.clone();
    glib::spawn_future_local(async move {
        b.upload_file_dialog().await;
    });
});
```

### 7b: Implement `upload_file_dialog()`

```rust
async fn upload_file_dialog(&self) {
    let root = self.widget.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::builder().title("Upload File").build();

    match dialog.open_future(root.as_ref()).await {
        Ok(file) => {
            if let Some(path) = file.path() {
                let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let current = self.current_path.borrow().clone();
                let remote_path = if current.is_empty() { filename.clone() } else { format!("{}/{}", current, filename) };

                self.status_label.set_text(&format!("Uploading {}...", filename));

                let svc = self.services.clone();
                let local = path.to_path_buf();
                let remote = remote_path.clone();
                let result = self.services.spawn(async move {
                    let token = svc.get_token().await;
                    let username = svc.get_username().await;
                    match (token, username) {
                        (Some(t), Some(u)) => svc.vospace.upload_file(&t, &u, &remote, &local).await,
                        _ => Err(crate::services::ApiError::Unauthorized),
                    }
                }).await;

                match result {
                    Ok(size) => {
                        self.status_label.set_text(&format!("Uploaded {} ({} bytes)", filename, size));
                        self.refresh().await;
                    }
                    Err(e) => {
                        self.status_label.set_text(&format!("Upload failed: {}", e));
                    }
                }
            }
        }
        Err(_) => {} // Cancelled
    }
}
```

### 7c: Add Download button to toolbar

```rust
let download_btn = gtk::Button::from_icon_name("folder-download-symbolic");
download_btn.set_tooltip_text(Some("Download selected file"));
download_btn.set_sensitive(false); // Enable when file is selected
toolbar.append(&download_btn);
```

Wire to selection changes in `list_box`:

```rust
// On row selected: enable/disable download button based on whether selection is a file
list_box.connect_row_selected(move |_, row| {
    if let Some(row) = row {
        let idx = row.index() as usize;
        let nodes = nodes_ref.borrow();
        if let Some(node) = nodes.get(idx) {
            download_btn.set_sensitive(!node.is_container());
        }
    } else {
        download_btn.set_sensitive(false);
    }
});
```

### 7d: Wire download button to save dialog

```rust
async fn download_selected(&self) {
    let selected_idx = self.file_list_box.selected_row().map(|r| r.index() as usize);
    let Some(idx) = selected_idx else { return; };
    let nodes = self.nodes.borrow();
    let Some(node) = nodes.get(idx) else { return; };
    if node.is_container() { return; }

    let filename = node.name.clone();
    drop(nodes);

    let root = self.widget.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::builder()
        .title("Save File")
        .initial_name(&filename)
        .build();

    match dialog.save_future(root.as_ref()).await {
        Ok(file) => {
            if let Some(local_path) = file.path() {
                let current = self.current_path.borrow().clone();
                let remote = if current.is_empty() { filename.clone() } else { format!("{}/{}", current, filename) };

                self.status_label.set_text(&format!("Downloading {}...", filename));

                let svc = self.services.clone();
                let local = local_path.clone();
                let result = self.services.spawn(async move {
                    let token = svc.get_token().await;
                    let username = svc.get_username().await;
                    match (token, username) {
                        (Some(t), Some(u)) => svc.vospace.download_file(&t, &u, &remote, &local).await,
                        _ => Err(crate::services::ApiError::Unauthorized),
                    }
                }).await;

                match result {
                    Ok(size) => {
                        // Format size nicely
                        let node = VoSpaceNode { size, ..Default::default() };
                        self.status_label.set_text(&format!("Downloaded {} ({})", filename, node.size_display()));
                    }
                    Err(e) => {
                        self.status_label.set_text(&format!("Download failed: {}", e));
                    }
                }
            }
        }
        Err(_) => {} // Cancelled
    }
}
```

### 7e: Add Delete button to toolbar

```rust
let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
delete_btn.set_tooltip_text(Some("Delete selected item"));
delete_btn.set_sensitive(false); // Enable when something is selected
toolbar.append(&delete_btn);
```

### 7f: Implement `create_folder_dialog()`

Replace the empty stub (line 241-242):

```rust
async fn create_folder_dialog(&self) {
    let root = self.widget.root().and_downcast::<gtk::Window>();
    let dialog = adw::MessageDialog::builder()
        .heading("New Folder")
        .body("Enter a name for the new folder")
        .modal(true)
        .build();
    if let Some(ref win) = root {
        dialog.set_transient_for(Some(win));
    }
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));

    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Folder name"));
    dialog.set_extra_child(Some(&entry));

    let response = dialog.choose_future().await;
    if response == "create" {
        let name = entry.text().to_string();
        if !validate_folder_name(&name) {
            self.status_label.set_text("Invalid folder name");
            return;
        }

        let current = self.current_path.borrow().clone();
        let folder_path = if current.is_empty() { name.clone() } else { format!("{}/{}", current, name) };

        let svc = self.services.clone();
        let result = self.services.spawn(async move {
            let token = svc.get_token().await;
            let username = svc.get_username().await;
            match (token, username) {
                (Some(t), Some(u)) => svc.vospace.create_folder(&t, &u, &folder_path).await,
                _ => Err(crate::services::ApiError::Unauthorized),
            }
        }).await;

        match result {
            Ok(()) => {
                self.status_label.set_text(&format!("Created folder '{}'", name));
                self.refresh().await;
            }
            Err(e) => {
                self.status_label.set_text(&format!("Create failed: {}", e));
            }
        }
    }
}

fn validate_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.trim().is_empty()
}
```

### 7g: Wire delete with confirmation

```rust
async fn delete_selected(&self) {
    let selected_idx = self.file_list_box.selected_row().map(|r| r.index() as usize);
    let Some(idx) = selected_idx else { return; };
    let nodes = self.nodes.borrow();
    let Some(node) = nodes.get(idx) else { return; };
    let name = node.name.clone();
    let is_folder = node.is_container();
    drop(nodes);

    let root = self.widget.root().and_downcast::<gtk::Window>();
    let dialog = adw::MessageDialog::builder()
        .heading(&format!("Delete '{}'?", name))
        .body(if is_folder {
            "This folder and all its contents will be permanently deleted."
        } else {
            "This file will be permanently deleted from your VOSpace storage."
        })
        .modal(true)
        .build();
    if let Some(ref win) = root {
        dialog.set_transient_for(Some(win));
    }
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

    let response = dialog.choose_future().await;
    if response == "delete" {
        let current = self.current_path.borrow().clone();
        let path = if current.is_empty() { name.clone() } else { format!("{}/{}", current, name) };

        let svc = self.services.clone();
        let result = self.services.spawn(async move {
            let token = svc.get_token().await;
            let username = svc.get_username().await;
            match (token, username) {
                (Some(t), Some(u)) => svc.vospace.delete_node(&t, &u, &path).await,
                _ => Err(crate::services::ApiError::Unauthorized),
            }
        }).await;

        match result {
            Ok(()) => {
                self.status_label.set_text(&format!("Deleted '{}'", name));
                self.refresh().await;
            }
            Err(e) => {
                self.status_label.set_text(&format!("Delete failed: {}", e));
            }
        }
    }
}
```

**Dependencies**: Steps 1-3.

---

## Step 8: UI -- Context Menu

**File**: `src/ui/vospace_browser.rs`

### 8a: Add right-click gesture to file list

```rust
let gesture = gtk::GestureClick::new();
gesture.set_button(3); // Right mouse button
gesture.connect_pressed(move |gesture, _n, x, y| {
    // Find which row was right-clicked
    // Show popover menu
});
self.file_list_box.add_controller(gesture);
```

### 8b: Build context menu popover

```rust
fn show_context_menu(&self, node: &VoSpaceNode, x: f64, y: f64, widget: &impl IsA<gtk::Widget>) {
    let menu = gio::Menu::new();

    if node.is_fits_file() {
        menu.append(Some("Open in FITS Viewer"), Some("storage.open-fits"));
    }
    if !node.is_container() {
        menu.append(Some("Download"), Some("storage.download"));
    }
    menu.append(Some("Copy VOSpace URI"), Some("storage.copy-uri"));
    menu.append(Some("Delete"), Some("storage.delete"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(widget);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}
```

### 8c: Wire actions

Use `gio::SimpleActionGroup` on the widget to handle `storage.open-fits`, `storage.download`, `storage.copy-uri`, `storage.delete`.

The "Copy VOSpace URI" action copies `node.uri` to the system clipboard:

```rust
let clipboard = widget.clipboard();
clipboard.set_text(&node.uri);
// Show status: "Copied URI to clipboard"
```

**Dependencies**: Step 6 (node list stored for lookup), Step 7 (download/delete logic).

---

## Step 9: UI -- Drag and Drop (Future Enhancement)

**File**: `src/ui/vospace_browser.rs`

### 9a: Add drop target

```rust
let drop_target = gtk::DropTarget::new(gtk4::gio::File::static_type(), gdk::DragAction::COPY);
drop_target.connect_drop(move |_, value, _x, _y| {
    if let Ok(file) = value.get::<gtk4::gio::File>() {
        if let Some(path) = file.path() {
            // Upload the dropped file
            glib::spawn_future_local(async move {
                browser.upload_file_from_path(&path).await;
            });
            return true;
        }
    }
    false
});
self.file_list_box.add_controller(drop_target);
```

### 9b: Visual drop overlay

Add a `gtk::Overlay` around the file list with a semi-transparent "Drop files here" label that appears during drag-over.

**Dependencies**: Step 7 (upload logic). This is low priority and can be deferred.

---

## Step 10: UI -- Status Bar Enhancement

**File**: `src/ui/vospace_browser.rs`

### 10a: Enhanced status display

In `refresh()`, after counting nodes, compute breakdown:

```rust
let folders = nodes.iter().filter(|n| n.is_container()).count();
let files = nodes.iter().filter(|n| !n.is_container()).count();
let total_size: u64 = nodes.iter().filter(|n| !n.is_container()).map(|n| n.size).sum();
let size_display = /* format total_size */;

if count >= 500 {
    self.status_label.set_text(&format!("500 items (listing limit reached)"));
} else {
    self.status_label.set_text(&format!("{} items | {} folders, {} files | {}", count, folders, files, size_display));
}
```

### 10b: Selection info

When a row is selected, update status bar to show selection info:

```rust
self.file_list_box.connect_row_selected(move |_, row| {
    if let Some(row) = row {
        let idx = row.index() as usize;
        let nodes = nodes_ref.borrow();
        if let Some(node) = nodes.get(idx) {
            status_label.set_text(&format!("Selected: {} ({})", node.name, node.size_display()));
        }
    }
});
```

**Dependencies**: Step 6.

---

## Step 11: Integration

### 11a: Wire OpenInFitsViewer

**File**: `src/ui/vospace_browser.rs`

Add callback field:

```rust
on_open_fits: Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>,
```

Add setter:

```rust
pub fn set_on_open_fits(&self, cb: impl Fn(PathBuf) + 'static) {
    *self.on_open_fits.borrow_mut() = Some(Box::new(cb));
}
```

For "Open in FITS Viewer" from context menu: download file to temp dir, then fire callback:

```rust
let temp_dir = std::env::temp_dir().join("verbinal_fits");
std::fs::create_dir_all(&temp_dir).ok();
let temp_path = temp_dir.join(&node.name);
// Download to temp_path, then:
if let Some(ref cb) = *self.on_open_fits.borrow() {
    cb(temp_path);
}
```

### 11b: Wire in main_window.rs

**File**: `src/ui/main_window.rs`

```rust
let fits_viewer_ref = fits_viewer.clone();
let view_stack_ref = view_stack.clone();
vospace_browser.set_on_open_fits(move |path| {
    fits_viewer_ref.load_from_path(&path);
    view_stack_ref.set_visible_child_name("fits");
});
```

### 11c: Register services

The `VoSpaceService` is already registered in `AppServices`. No additional registration needed.

**Dependencies**: Steps 7, 8.

---

## Implementation Order

| Step | Description | File(s) | Effort | Dependencies |
|------|-------------|---------|--------|-------------|
| 1 | Extend VoSpaceNode model | `src/models/vospace_node.rs` | 30 min | None |
| 2 | Complete VoSpaceParser | `src/helpers/vospace_parser.rs` | 20 min | Step 1 |
| 3 | Complete VoSpaceService | `src/services/vospace_service.rs` | 30 min | None |
| 4 | Config endpoints | `src/config.rs` | 15 min | None |
| 5 | UI - Breadcrumb bar | `src/ui/vospace_browser.rs` | 45 min | None |
| 6 | UI - File list improvements | `src/ui/vospace_browser.rs` | 1 hr | Steps 1, 2 |
| 7 | UI - Toolbar completion | `src/ui/vospace_browser.rs` | 2.5 hr | Steps 1-3 |
| 8 | UI - Context menu | `src/ui/vospace_browser.rs` | 1 hr | Steps 6, 7 |
| 9 | UI - Drag and drop | `src/ui/vospace_browser.rs` | 1 hr | Step 7 (defer) |
| 10 | UI - Status bar | `src/ui/vospace_browser.rs` | 30 min | Step 6 |
| 11 | Integration | `src/ui/vospace_browser.rs`, `src/ui/main_window.rs` | 45 min | Steps 7, 8 |

**Total estimate**: ~9 hours.

Steps 1-4 can be done in parallel (model/service layer). Steps 5, 6 can be done in parallel. Step 7 is the largest single step. Step 9 (drag-and-drop) is lower priority and can be deferred to a later iteration.
