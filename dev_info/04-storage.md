# 04 - Storage Module Specification

## Purpose

The Storage module is a VOSpace file browser that lets authenticated users browse, upload, download, create folders, and delete files in their CANFAR cloud storage. It operates against the IVOA VOSpace 2.0 REST API exposed by the CADC `arc` service.

## Layout

Vertical stack: toolbar -> breadcrumb bar -> file list -> status bar.

```
+---------------------------------------------------------------+
| [Up] [Refresh] [New Folder] [Upload] [Download] [Delete]     |  <- toolbar
+---------------------------------------------------------------+
| / home / username / data / observations /                     |  <- breadcrumb
+---------------------------------------------------------------+
| [folder-icon]  calibrations/               --     2024-01-15  |
| [folder-icon]  raw_data/                   --     2024-01-10  |
| [science-icon] ngc1234_cal.fits         145.3 MB  2024-01-14  |
| [code-icon]    reduce.py                  2.1 KB  2024-01-13  |
| [table-icon]   catalog.csv              512.0 KB  2024-01-12  |
| [image-icon]   preview.jpg               89.4 KB  2024-01-11  |
| [doc-icon]     notes.pdf                  1.2 MB  2024-01-10  |
| [archive-icon] backup.tar.gz            23.5 MB   2024-01-09  |
+---------------------------------------------------------------+
| 8 items | 3 folders, 5 files | 171.5 MB total                |  <- status bar
+---------------------------------------------------------------+
```

### Toolbar

`gtk::Box` horizontal with 8px spacing, 12px margins.

| Button | Icon | Tooltip | Behavior |
|--------|------|---------|----------|
| Up | `go-up-symbolic` | Go to parent folder | Navigate to parent path segment |
| Refresh | `view-refresh-symbolic` | Refresh current listing | Re-fetch current directory |
| New Folder | `folder-new-symbolic` | Create new folder | Show folder name dialog |
| Upload | `document-send-symbolic` | Upload file | Show file chooser, then PUT |
| Download | `folder-download-symbolic` | Download selected file | Show save dialog, then GET |
| Delete | `user-trash-symbolic` | Delete selected item | Show confirmation, then DELETE |

Upload and Download buttons are sensitive only when a file (not folder) is selected. Delete is sensitive when any item is selected.

### Breadcrumb Bar

`gtk::Box` horizontal containing clickable `gtk::Button` segments styled as flat link buttons.

```rust
fn build_breadcrumbs(path: &str, on_navigate: impl Fn(&str)) {
    // Always start with "/" (home root)
    // Split path by "/" and create a button for each segment
    // Clicking segment N navigates to path segments 0..=N joined by "/"
    // Last segment is a plain label (not clickable)
}
```

Example for path `data/observations/2024`:
- `[/]` -> navigate to `""`
- `[data]` -> navigate to `"data"`
- `[observations]` -> navigate to `"data/observations"`
- `2024` (label, not clickable)

### File List

`gtk::ListBox` with `boxed-list` CSS class, inside `gtk::ScrolledWindow`. Each row is an `adw::ActionRow`.

Columns represented within the ActionRow:
- **Prefix icon**: `gtk::Image` (16px), mapped by node type and file extension.
- **Title**: File/folder name.
- **Subtitle**: Size display for files (`"145.3 MB"`), `"Folder"` for containers.
- **Suffix labels**: Date column as a dim `gtk::Label`.
- **Suffix buttons**: Download button (files only) and Delete button, both flat style.

### Status Bar

`gtk::Label` with `dim-label` and `caption` CSS classes. Displays: `"{total} items | {folders} folders, {files} files"`.

## Data Model

### VoSpaceNode (existing, extended)

The existing `VoSpaceNode` struct in `src/models/vospace_node.rs` is extended with additional fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoSpaceNode {
    pub name: String,
    pub uri: String,
    pub node_type: NodeType,      // Container, Data, Link
    pub size: u64,                // ivo://ivoa.net/vospace/core#length
    pub date: Option<String>,     // ivo://ivoa.net/vospace/core#date
    pub content_type: Option<String>,  // ivo://ivoa.net/vospace/core#type
    pub is_public: bool,          // ivo://cadc.nrc.ca/vospace/core#ispublic
}
```

### Icon Mapping

Map file extension to icon name for the prefix image:

```rust
fn icon_for_node(node: &VoSpaceNode) -> &'static str {
    if node.is_container() {
        return "folder-symbolic";
    }
    let ext = node.name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "fits" | "fit" | "fts" | "fz" => "image-x-generic-symbolic",  // science/FITS
        "py" | "sh" | "bash" | "r" | "jl" | "rs" => "text-x-script-symbolic",  // code
        "csv" | "tsv" | "dat" | "cat" | "vot" => "x-office-spreadsheet-symbolic",  // table
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "svg" => "image-x-generic-symbolic",  // image
        "pdf" | "doc" | "docx" | "odt" | "txt" | "md" | "tex" | "rtf" => "x-office-document-symbolic",  // document
        "tar" | "gz" | "bz2" | "xz" | "zip" | "7z" | "rar" => "package-x-generic-symbolic",  // archive
        "ipynb" => "accessories-text-editor-symbolic",  // notebook
        _ => "text-x-generic-symbolic",  // default
    }
}
```

## StorageService API

The existing `VoSpaceService` at `src/services/vospace_service.rs` implements the core operations. The following documents the full REST API mapping needed for feature completeness.

### List Nodes

```
GET {api_base_url}/arc/nodes/home/{username}/{path}?detail=max
Accept: text/xml
Authorization: Bearer {token}
```

Response: VOSpace 2.0 XML containing a `<node>` with child `<nodes>`.

Query parameter `detail=max` returns all properties. Without it, properties may be omitted.

Maximum 500 child nodes returned per request. If a directory contains more than 500 items, only the first 500 are returned. The status bar should indicate when the limit is hit: `"500 items (listing limit reached)"`.

### Upload File

```
PUT {api_base_url}/arc/files/home/{username}/{path}/{filename}
Content-Type: application/octet-stream
Authorization: Bearer {token}
Body: raw file bytes
```

The PUT endpoint for files (`/arc/files/home/`) accepts raw bytes. No XML wrapping needed.

For large files, use streaming upload via `reqwest::Body::wrap_stream()` to avoid loading entire file into memory.

### Download File

```
GET {api_base_url}/arc/files/home/{username}/{remote_path}
Authorization: Bearer {token}
```

Response: raw file bytes. Stream to disk using `tokio::io::copy` from response body to `tokio::fs::File`.

### Create Folder

```
PUT {api_base_url}/arc/nodes/home/{username}/{path}/{folder_name}
Content-Type: text/xml
Authorization: Bearer {token}
Body: ContainerNode XML
```

XML body:
```xml
<vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0"
          xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
          uri="vos://cadc.nrc.ca~arc/home/{username}/{path}/{folder_name}"
          xsi:type="vos:ContainerNode">
    <vos:properties/>
    <vos:nodes/>
</vos:node>
```

### Delete Node

```
DELETE {api_base_url}/arc/nodes/home/{username}/{path}
Authorization: Bearer {token}
```

Deletes files and folders (folders must be empty, or the server handles recursive deletion). The server returns 200 on success, 404 if not found, 409 if container is not empty (some servers).

## VoSpaceParser

Existing parser at `src/helpers/vospace_parser.rs`. The parser handles the VOSpace 2.0 XML namespace `http://www.ivoa.net/xml/VOSpace/v2.0`.

### XML Namespace Handling

The CADC VOSpace service returns XML with the `vos:` prefix bound to `http://www.ivoa.net/xml/VOSpace/v2.0`. The `roxmltree` crate handles namespaces transparently -- `node.tag_name().name()` returns the local name without prefix.

### Property URIs

| Property URI | Field | Notes |
|-------------|-------|-------|
| `ivo://ivoa.net/vospace/core#length` | `size` | File size in bytes |
| `ivo://ivoa.net/vospace/core#date` | `date` | ISO 8601 timestamp |
| `ivo://ivoa.net/vospace/core#type` | `content_type` | MIME type |
| `ivo://cadc.nrc.ca/vospace/core#ispublic` | `is_public` | `"true"` or `"false"` |

The parser matches properties by checking if the URI contains the key substring (`"length"`, `"date"`, `"type"`, `"ispublic"`) rather than exact URI match, for resilience against URI variations.

### Node Type Detection

The `xsi:type` attribute on `<node>` elements determines the node type:
- `vos:ContainerNode` -> `NodeType::Container`
- `vos:DataNode` -> `NodeType::Data`
- `vos:LinkNode` -> `NodeType::Link`

The attribute is in the XSI namespace (`http://www.w3.org/2001/XMLSchema-instance`). The parser tries both the namespaced attribute and a plain `type` attribute as fallback.

## Navigation

### Double-click / Row Activation

```rust
file_list_box.connect_row_activated(move |_, row| {
    let idx = row.index() as usize;
    // Look up node at idx in the current node list
    // If node.is_container() -> navigate into folder
    // If node is a file -> no action (or open preview)
});
```

Navigate into folder: set `current_path` to `"{current_path}/{folder_name}"`, then call `refresh()`.

### Breadcrumb Navigation

Clicking a breadcrumb segment navigates to the path up to and including that segment. Example: clicking `"data"` in path `"data/observations/2024"` sets `current_path` to `"data"` and refreshes.

### Up Button

```rust
fn go_up(&self) {
    let path = self.current_path.borrow().clone();
    if path.is_empty() {
        return;  // Already at root
    }
    let new_path = match path.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => String::new(),  // Go to root
    };
    *self.current_path.borrow_mut() = new_path;
    self.refresh().await;
}
```

## Sorting

Default sort: folders first, then alphabetically by name (case-insensitive). This is already implemented in `vospace_parser::parse_nodes()`.

Extended sorting by column click (future enhancement):

```rust
#[derive(Clone, Copy)]
enum SortColumn {
    Name,
    Size,
    Date,
}

#[derive(Clone, Copy)]
enum SortDirection {
    Ascending,
    Descending,
}

fn sort_nodes(nodes: &mut Vec<VoSpaceNode>, column: SortColumn, direction: SortDirection) {
    nodes.sort_by(|a, b| {
        // Folders always first regardless of sort
        let folder_cmp = b.is_container().cmp(&a.is_container());
        if folder_cmp != std::cmp::Ordering::Equal {
            return folder_cmp;
        }
        let cmp = match column {
            SortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Date => a.date.cmp(&b.date),
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}
```

Column headers can be implemented as `gtk::Button` widgets in a horizontal box above the list, styled flat. Clicking toggles ascending/descending.

## Upload

### File Chooser

```rust
async fn upload_file_dialog(&self, parent: &impl IsA<gtk::Widget>) {
    let dialog = gtk::FileDialog::builder()
        .title("Upload File")
        .build();
    match dialog.open_future(root.as_ref()).await {
        Ok(file) => {
            if let Some(path) = file.path() {
                self.upload_file(&path).await;
            }
        }
        Err(_) => {}  // Cancelled
    }
}
```

### Upload Execution

1. Read local file path and extract filename.
2. Construct remote path: `"{current_path}/{filename}"`.
3. Show progress indication (spinner in toolbar or status bar text `"Uploading {filename}..."`).
4. PUT file bytes to `/arc/files/home/{username}/{remote_path}`.
5. On success: refresh listing, show toast `"Uploaded {filename}"`.
6. On error: show toast with error message.

### Drag and Drop (Future Enhancement)

Add a `gtk::DropTarget` to the file list area:
- Accept `text/uri-list` and `application/vnd.portal.filedescriptor` MIME types.
- On drop: show a visual overlay (`gtk::Overlay` with semi-transparent background and "Drop files here" label).
- Extract file paths from URIs, upload each sequentially.

## Download

### File Download Flow

1. User selects a file row and clicks Download (toolbar or row suffix button).
2. Show save dialog with suggested filename (`node.name`):
   ```rust
   let dialog = gtk::FileDialog::builder()
       .title("Save File")
       .initial_name(&node.name)
       .build();
   ```
3. On path chosen: stream download to local file.
4. Show progress in status bar: `"Downloading {filename}..."`.
5. On success: show toast `"Downloaded {filename} ({size_display})"`.
6. On error: show toast with error message.

### Stream Copy

```rust
pub async fn download_file(
    &self,
    token: &str,
    username: &str,
    remote_path: &str,
    local_path: &Path,
) -> Result<u64, ApiError> {
    let url = self.endpoints.vospace_files_url(username, remote_path);
    let resp = self.client.get(&url)
        .bearer_auth(token)
        .send()
        .await?;
    let resp = check_response(resp).await?;
    let bytes = resp.bytes().await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let len = bytes.len() as u64;
    std::fs::write(local_path, &bytes)
        .map_err(|e| ApiError::Network(format!("Write error: {}", e)))?;
    Ok(len)
}
```

For large files (future improvement): use `resp.chunk()` loop with progress reporting.

## Context Menu

Right-click on a file row shows a `gtk::PopoverMenu` with:

| Action | Icon | Condition |
|--------|------|-----------|
| Open in FITS Viewer | `image-x-generic-symbolic` | File extension is .fits/.fit/.fts |
| Download | `folder-download-symbolic` | Any file (not folder) |
| Copy VOSpace URI | `edit-copy-symbolic` | Any item |
| Delete | `user-trash-symbolic` | Any item |

### Open in FITS Viewer

1. Download file to a temporary path: `{std::env::temp_dir()}/verbinal_fits/{filename}`.
2. Fire callback to main window: `on_open_fits(temp_path)`.
3. Main window calls `fits_viewer.load_from_path(&temp_path)` and switches to FITS tab.

### Copy VOSpace URI

Copy `node.uri` to clipboard:
```rust
let clipboard = widget.clipboard();
clipboard.set_text(&node.uri);
// Show toast: "Copied URI to clipboard"
```

## Folder Creation

### Dialog

```rust
async fn create_folder_dialog(&self) {
    // Use adw::MessageDialog or adw::Window with an entry
    let dialog = adw::MessageDialog::builder()
        .heading("New Folder")
        .body("Enter a name for the new folder")
        .transient_for(&window)
        .modal(true)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);

    // Add entry widget as extra child
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Folder name"));
    dialog.set_extra_child(Some(&entry));

    let response = dialog.choose_future().await;
    if response == "create" {
        let name = entry.text().to_string();
        if validate_folder_name(&name) {
            self.create_folder(&name).await;
        }
    }
}
```

### Name Validation

```rust
fn validate_folder_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    // Reject path traversal and invalid characters
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return false;
    }
    // Reject names that are only whitespace
    if name.trim().is_empty() {
        return false;
    }
    true
}
```

On validation failure, show inline error text on the entry widget or a toast with the reason.

### Folder Creation Execution

1. Validate name.
2. Construct path: `"{current_path}/{folder_name}"`.
3. PUT ContainerNode XML to `/arc/nodes/home/{username}/{path}`.
4. On success: refresh listing, show toast `"Created folder '{name}'"`.
5. On error 409 (conflict/already exists): show toast `"Folder '{name}' already exists"`.

## Delete

### Confirmation

```rust
let dialog = adw::MessageDialog::builder()
    .heading(&format!("Delete '{}'?", node.name))
    .body(if node.is_container() {
        "This folder and all its contents will be permanently deleted."
    } else {
        "This file will be permanently deleted from your VOSpace storage."
    })
    .transient_for(&window)
    .modal(true)
    .build();
dialog.add_response("cancel", "Cancel");
dialog.add_response("delete", "Delete");
dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
```

### Delete Execution

1. Extract node path from URI (strip the `vos://cadc.nrc.ca~arc/home/` prefix to get the relative path).
2. DELETE `/arc/nodes/home/{username}/{path}`.
3. On success: refresh listing, show toast `"Deleted '{name}'"`.
4. On error: show toast with error message.

## Module Files

The Storage module is largely implemented across existing files. Changes needed:

| File | Status | Changes |
|------|--------|---------|
| `src/models/vospace_node.rs` | Existing | Add `is_public` field |
| `src/services/vospace_service.rs` | Existing | Add `upload_file()` method |
| `src/helpers/vospace_parser.rs` | Existing | Parse `ispublic` property |
| `src/ui/vospace_browser.rs` | Existing | Implement remaining stubs: `on_row_activated`, `create_folder_dialog`, upload, download, delete, context menu, breadcrumbs, sorting |

## GTK4/Adwaita Widget Mapping

| Concept | Widget |
|---------|--------|
| Toolbar | `gtk::Box` horizontal with `gtk::Button` children |
| Breadcrumbs | `gtk::Box` horizontal with flat `gtk::Button` segments |
| File list | `gtk::ListBox` with `adw::ActionRow` rows |
| File icon | `gtk::Image` (16px) as row prefix |
| Save/Open dialogs | `gtk::FileDialog` |
| Folder name input | `adw::MessageDialog` with `gtk::Entry` extra child |
| Delete confirmation | `adw::MessageDialog` |
| Context menu | `gtk::PopoverMenu` attached to right-click gesture |
| Progress indication | Status bar label text + `gtk::Spinner` |
| Notifications | `adw::Toast` via `adw::ToastOverlay` |

## Error Handling

- **401 Unauthorized**: Show toast `"Session expired. Please log in again."`. This triggers the same behavior as the main window's session expiry handling.
- **404 Not Found**: On navigate, show toast `"Folder not found"`, navigate back to root.
- **409 Conflict**: On folder creation, show `"Folder already exists"`.
- **Network errors**: Show toast with the error message from `ApiError` display.
- **Empty directory**: Show the placeholder label `"This folder is empty"` (via `ListBox::set_placeholder`).
- **Large directories (>500 items)**: Display status bar warning. Consider adding a note in the status bar: `"Showing first 500 items"`.
