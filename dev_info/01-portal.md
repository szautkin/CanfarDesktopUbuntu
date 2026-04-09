# Portal Module Specification

> Module status: **Implemented** in Rust/GTK4 (libadwaita)
> Covers: Session management, launch, monitoring, storage quota, platform load, recent launches

---

## 1. Dashboard Layout

The Portal page is the main "Home" / "Dashboard" view, shown after login. It uses a 2-row GTK Grid layout.

```
+-------------------------------------------------------+
| Row 0, Col 0 (span 1)       | Row 0, Col 1 (span 1)  |
| SessionListView              | StorageQuotaView        |
| (Active Sessions)            | (User Home Storage)     |
|                              |                         |
+------------------------------+-------------------------+
| Row 1, Col 0 (span 1)       | Row 1, Col 1 (span 1)  |
| LaunchFormView               | RecentLaunchesView      |
| (Launch Session)             | PlatformLoadView        |
|                              |                         |
+------------------------------+-------------------------+
```

- The grid has `row_homogeneous = false`, `column_homogeneous = true`.
- SessionListView is `vexpand = true`.
- The bottom-right cell is a vertical `gtk::Box` containing RecentLaunchesView on top and PlatformLoadView below.
- All cards use the `card` CSS class, with 8px margins on each side.

The DashboardView struct owns all five sub-views as `Rc<T>` and an `Arc<AppServices>`.

### Loading Sequence

On `load_data()`, the following happen concurrently:
1. `session_list.refresh()` -- fetches sessions
2. `storage_quota.refresh()` -- fetches VOSpace quota
3. `platform_load.refresh()` -- fetches cluster stats
4. `launch_form.load_images()` -- fetches image catalog + context + repositories
5. `recent_launches.refresh()` -- loads from local JSON file

After all load, `update_session_limits()` is called to sync the launch button state.

---

## 2. Session List

### Data Source

- **Endpoint**: `GET {base}/skaha/v1/session`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: `Vec<SkahaSessionResponse>` (JSON array)

The response is deserialized into `Vec<SkahaSessionResponse>`, then mapped to `Vec<Session>` via `From<SkahaSessionResponse>`.

### Filtering

Headless sessions are NOT filtered out in the current implementation. All sessions returned by the API are displayed. The session list shows every session regardless of type.

### Session Card Display

Each session is rendered as a `SessionCard` widget (220px min width, vertical layout):

```
+----------------------------------+
|        [Type Icon 48px]          |
|                                  |
|  session-name         [STATUS]   |
|  image-name:tag (dim)            |
|  > Start: Jan 15 10:00           |
|  ! Expiry: Jan 22 10:00          |
|  CPU: 2  RAM: 8G  [FLEX]        |
|                                  |
|          [Open][Renew][Info][Del]|
+----------------------------------+
```

**Layout details:**
- Type icon: 48px, centered at top, loaded from embedded JPG/PNG assets based on `session_type`
- Name + Status badge in a horizontal box
  - Name: `heading` CSS class, `ellipsize = End`
  - Status badge: `caption` + `status-{status}` CSS class
- Image name: shows only the trailing `name:tag` portion (after last `/`)
- Times row: start icon (`media-playback-start-symbolic`) + formatted time, expiry icon (`alarm-symbolic`) + formatted time
- Resource row: `CPU: {cores}`, `RAM: {ram}`, optionally `GPU: {gpus}` (only if != "0"), optionally `FLEX` badge (if `is_fixed_resources == false`)
- Action buttons row (right-aligned): Open, Renew, Events, Delete

**Time formatting**: Parsed as RFC3339 via `chrono::DateTime::parse_from_rfc3339`, displayed as `"%b %d %H:%M"` (e.g., "Jan 15 10:00"). Falls back to first 16 chars if parse fails.

### Status Badge Colors (CSS classes)

| Status       | CSS Class           | Color  |
|-------------|---------------------|--------|
| Running     | `status-running`    | Green  |
| Pending     | `status-pending`    | Amber  |
| Failed      | `status-failed`     | Red    |
| Terminating | `status-terminating`| Gray   |

Status comparison is case-insensitive (`eq_ignore_ascii_case`).

### Session Count Display

The header shows "{N} session{s}" label, updated on each refresh.

### Auto-Poll While Pending

When any session has `is_pending() == true` after a refresh:
1. A countdown label becomes visible in the header
2. Counts down from 15 to 1 (one tick per second), displaying "refresh in {N}s"
3. At 0, shows "refreshing..." and re-fetches sessions
4. After fetch, rebuilds all cards
5. If still pending, repeats the loop
6. If no more pending, hides countdown and stops polling

Constant: `AUTO_REFRESH_SECS = 15`

### Card Header Component

All dashboard panels share a common `card_header(title)` function that returns `(gtk::Box, gtk::Spinner, gtk::Button)`:
- Title label (title-4 CSS class, left-aligned, hexpand)
- Spinner (initially hidden)
- Refresh button (view-refresh-symbolic icon)

---

## 3. Session Card Actions

### Open in Browser
- **Trigger**: Click "Open in browser" button (web-browser-symbolic)
- **Condition**: Button is only sensitive when `session.is_running()` is true
- **Action**: `open::that(&session.connect_url)` -- opens the connect URL in the default system browser

### Delete Session
- **Trigger**: Click trash button (user-trash-symbolic, destructive-action CSS class)
- **Flow**:
  1. Show `adw::MessageDialog` with title "Delete Session", body "Are you sure you want to delete session '{name}'?\n\nThis action cannot be undone."
  2. Two responses: "Cancel" (default) and "Delete" (destructive appearance)
  3. If user clicks "Delete":
     - **Endpoint**: `DELETE {base}/skaha/v1/session/{session_id}`
     - **Auth**: `Authorization: Bearer {token}`
     - Wait 3 seconds (for backend to process)
     - Refresh session list
     - Update session limits

### Renew Session
- **Trigger**: Click refresh button (view-refresh-symbolic)
- **Endpoint**: `POST {base}/skaha/v1/session/{session_id}?action=renew`
- **Auth**: `Authorization: Bearer {token}`
- **Body**: Empty
- On success, refresh the session list immediately

### View Events/Logs
- **Trigger**: Click info button (dialog-information-symbolic)
- **Flow**:
  1. Fetch events and logs in parallel
  2. Show a modal `gtk::Window` (600x500px) with a `gtk::Notebook` containing two tabs

**Events tab**:
- **Endpoint**: `GET {base}/skaha/v1/session/{session_id}?view=events`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: Plain text
- Displayed in a read-only, monospace `gtk::TextView` with word wrap

**Logs tab**:
- **Endpoint**: `GET {base}/skaha/v1/session/{session_id}?view=logs`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: Plain text
- Same display as events tab

Both show "No events/logs available" if the response is empty.

---

## 4. Session Launch

### Launch Form Structure

The LaunchFormView has two tabs (gtk::Notebook): **Standard** and **Advanced**.

#### Standard Tab

Uses `adw::PreferencesGroup` rows:

1. **Session Type** (DropDown): `["notebook", "desktop", "carta", "contributed", "firefly"]`
2. **Image Registry** (DropDown): Populated dynamically from images filtered by selected type
3. **Project** (DropDown): Populated dynamically from images filtered by type + registry
4. **Container Image** (DropDown): Populated dynamically from images filtered by type + registry + project
5. **Session Name** (EntryRow): Auto-generated as `"{type}{N+1}"` where N = count of sessions of that type
6. **Fixed Resources** toggle (Switch): When ON, shows the ResourceSelector
7. **ResourceSelector** (visible only when toggle is ON): SpinButtons for cores, RAM (GB), GPUs

#### Advanced Tab

Uses two `adw::PreferencesGroup` sections:

**Custom Container Image:**
1. **Session Type** (DropDown): `["notebook", "desktop", "carta", "contributed", "firefly", "headless"]` (note: includes "headless")
2. **Image Registry** (DropDown): Populated from `/v1/repository` API endpoint
3. **Image** (EntryRow): Free-text input for `project/name:tag`

**Registry Authentication:**
1. **Username** (EntryRow)
2. **Token or Password** (PasswordEntryRow)

### Cascading Dropdown Logic

When **type** changes:
1. Filter all images to those with matching type
2. Extract unique registries -> populate Registry dropdown
3. Trigger registry change handler

When **registry** changes:
1. Filter images by type + registry
2. Extract unique projects -> populate Project dropdown
3. Try to select the project that contains the default image for this type
4. Trigger project change handler

When **project** changes:
1. Filter images by type + registry + project
2. Populate Image dropdown (sorted by version descending)
3. Try to select the default image for this type
4. Auto-generate session name

### Default Images Per Type

```
notebook    -> astroml:latest
desktop     -> desktop:latest
carta       -> carta:latest
contributed -> astroml-vscode:latest
firefly     -> firefly:2025.2
```

Matching logic: checks if `image.id` ends with the default name, OR if `image.display_name` equals the default name.

### Auto-Name Generation

Format: `{session_type}{count + 1}` where count = number of currently active sessions of that type.

Example: If there are 2 notebook sessions running, new name = "notebook3".

### Resource Selection

**ResourceSelector** widget provides three SpinButton rows:
- **CPU Cores**: range from context API, default from context API (fallback: min=1, max=16, default=2)
- **RAM (GB)**: range from context API, default from context API (fallback: min=1, max=256, default=8)
- **GPUs**: range from context API (fallback: min=0, max=4, default=0)

When the "Fixed Resources" toggle is OFF, the app uses config defaults: `default_cores = 2`, `default_ram = 8`, `gpus = 0`.

### Launch Execution

1. Validate: image must be selected/entered, name must be non-empty, session limit must not be reached
2. Build `SessionLaunchParams`
3. Disable launch button, show "Launching session..."

**Standard tab launch:**
- Image ID comes from the parsed image catalog
- No registry credentials

**Advanced tab launch:**
- Image = `{registry_host}/{custom_path}` (prepends registry host to the entered path)
- Registry credentials if provided

**API Call:**
- **Endpoint**: `POST {base}/skaha/v1/session`
- **Auth**: `Authorization: Bearer {token}`
- **Content-Type**: `application/x-www-form-urlencoded`
- **Body fields** (form-encoded):
  - `name` (String)
  - `image` (String, full image ID e.g. `images.canfar.net/skaha/notebook:1.0`)
  - `type` (String, e.g. "notebook")
  - `cores` (String, e.g. "2")
  - `ram` (String, e.g. "8")
  - `gpus` (String, only included if > 0)
  - `cmd` (String, optional)
  - `env` (String, optional)
- **Registry Auth Header** (optional): `x-skaha-registry-auth: {base64(username:password)}`
  - Only included when `registry_username` is set
  - Uses standard Base64 encoding of `"{username}:{secret}"`

**Response handling:**
- Success: Response body is either a JSON array of session IDs `["abc123"]` or a plain string session ID
- The first ID is extracted from the array
- Show launch result dialog

### Launch Result Dialog

`adw::MessageDialog` showing:
- Heading: `"{type} {name} launched!"` (success) or `"{type} Launch failed"` (error)
- Body: "Session is starting. It will appear in Active Sessions shortly." (success) or error message
- Extra child: resource summary line `"{image} . CPU: {cores} . RAM: {ram}G . GPU: {gpus}"`
- Auto-closes after 2 seconds on success

### Post-Launch

On success:
1. Save to recent launches (via `RecentLaunchService`)
2. Call `on_launched` callback
3. Wait 2 seconds, then refresh session list and recent launches

---

## 5. Session Limit

- **Constant**: `MAX_SESSIONS = 3`
- When `session_count >= 3`:
  - Launch button disabled
  - Status label shows "Session limit reached (max 3 concurrent sessions)"
  - Relaunch buttons in Recent Launches disabled
- Checked on every session list refresh and after delete/launch operations

---

## 6. Platform Load

### Data Source

- **Endpoint**: `GET {base}/skaha/v1/session?view=stats`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: `SkahaStatsResponse` (may be a single object or wrapped in an array)

The service tries to parse as a single object first, then as an array (taking the first element).

### Display

The PlatformLoadView card shows:
1. **CPU metric bar**: "Available CPUs: {available} / {total}" with a progress bar showing used fraction
2. **RAM metric bar**: "Available RAM: {available} / {total} GB" with a progress bar
3. **Instance counts**: "Instances: {total} total ({sessions} sessions, {desktop_apps} desktop apps, {headless} headless)"
4. **Last update timestamp**: "last update: {YYYY-MM-DD HH:MM} UTC"

### MetricBar Component

A reusable widget with:
- Heading label (`caption-heading` CSS class)
- `gtk::ProgressBar` showing `used / total` as fraction
- Color classes: `error` if used > 90%, `warning` if used > 70%

### RAM Parsing

RAM strings from the API use suffixes: `G`/`Gi` (gigabytes), `M`/`Mi` (megabytes, divide by 1024), `T`/`Ti` (terabytes, multiply by 1024). Falls back to parsing as a plain number.

CPU values can be numbers or strings; the code handles both via `serde_json::Value`.

---

## 7. Storage Quota

### Data Source

- **Endpoint**: `GET {base}/arc/nodes/home/{username}?limit=0`
- **Auth**: `Authorization: Bearer {token}`
- **Accept**: `text/xml`
- **Response**: VOSpace XML document

### XML Parsing

Parses the VOSpace XML looking for `<property>` elements:
- URI containing `quota` -> `quota_bytes` (u64)
- URI containing `length` -> `used_bytes` (u64)
- URI containing `date` -> `last_update` (String)

### Display

The StorageQuotaView card shows:
1. **Progress bar**: fraction = `used_bytes / quota_bytes`
   - CSS class `error` if usage > 90% (`.is_warning()`)
   - CSS class `warning` if usage > 70%
2. "Used: {X.X} GB"
3. "Quota: {X.X} GB"
4. "Usage: {X.X}%"
5. "last update: {date}" (if available)

### Warning Threshold

`StorageQuota::is_warning()` returns `true` when `usage_percent() > 90.0`.

---

## 8. Recent Launches

### Persistence

- **File**: `{XDG_DATA_HOME}/Verbinal/recent_launches.json` (via `directories::ProjectDirs`)
- **Format**: JSON array of `RecentLaunch` objects
- **Max entries**: 10 (`MAX_RECENT = 10`)

### Save Behavior

When saving a new launch:
1. Remove any existing entry with the same `image` AND `session_type` (dedup)
2. Insert at the front
3. Truncate to 10 entries
4. Write to disk as pretty-printed JSON

### Display

The RecentLaunchesView card shows:
- Header: "Recent Launches" title + "Clear history" button (edit-clear-all-symbolic)
- Filter: `gtk::SearchEntry` with placeholder "Filter..."
- List: `gtk::ListBox` (boxed-list CSS class) with `adw::ActionRow` items

Each row shows:
- Prefix: session type icon (32px)
- Title: launch name
- Subtitle: `"{Type Display} | {short image name} | CPU:{cores} RAM:{ram}G"`
- Suffix: Relaunch button (media-playback-start-symbolic) + Remove button (edit-delete-symbolic)

### Filtering

Filters on `name`, `session_type`, and `image` (case-insensitive contains match against filter text).

### Relaunch

When relaunch is clicked:
1. Generate new name: `{session_type}{type_count + 1}`
2. Build `SessionLaunchParams` with the saved image, cores, ram, gpus
3. Launch the session (same API call as standard launch)
4. Show the launch result dialog
5. If successful, wait 2s, refresh session list and recent launches

### Type Display Names

```
notebook    -> "Notebook"
desktop     -> "Desktop"
carta       -> "CARTA"
contributed -> "Contributed"
firefly     -> "Firefly"
headless    -> "Headless"
```

---

## 9. Image Catalog

### Data Sources

**Available images:**
- **Endpoint**: `GET {base}/skaha/v1/image`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: `Vec<RawImage>` where `RawImage = { id: String, types: Vec<String> }`
- **Caching**: Results cached for 300 seconds (5 minutes) in `ImageService`

**Available repositories:**
- **Endpoint**: `GET {base}/skaha/v1/repository`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: `Vec<String>` (list of registry hostnames)

**Resource context:**
- **Endpoint**: `GET {base}/skaha/v1/context`
- **Auth**: `Authorization: Bearer {token}`
- **Response**: `SessionContext` JSON (see model below)

### Image ID Parsing

The `parse_image_id` function splits an image ID like `images.canfar.net/skaha/notebook:1.0` into:
- `registry`: `images.canfar.net`
- `project`: `skaha`
- `name`: `notebook`
- `version`: `1.0`

Handles various depths:
- `notebook` -> registry="", project="", name="notebook", version="latest"
- `skaha/notebook:latest` -> registry="", project="skaha", name="notebook", version="latest"
- `images.canfar.net/skaha/notebook:1.0` -> full parse
- `registry.example.com/org/sub/image:v2.3` -> deep path, project="org/sub"

If no `:` found, version defaults to `"latest"`.

### ImageParser Helper

Provides static filtering methods:
- `registries_for_type(images, type)` -> unique sorted registries
- `projects_for_type_and_registry(images, type, registry)` -> unique sorted projects
- `images_for_type_registry_and_project(images, type, registry, project)` -> images sorted by version descending
- `available_types(images)` -> types sorted in canonical order: notebook, desktop, carta, contributed, firefly, headless

---

## 10. Notifications

### Desktop Notifications

The `NotificationService` sends `gio::Notification` via the GTK application object. It maintains a `HashSet<String>` of already-sent notification keys to prevent duplicates.

**Notification types:**

| Event | Title | Priority | Key Format |
|-------|-------|----------|------------|
| Session Ready | "{Type} Session Ready" | Normal | `ready:{session_id}` |
| Session Failed | "Session Failed" | Urgent | `failed:{session_id}` |
| Session Expiring | "Session Expiring Soon" | High | `expiring:{session_id}` |

Notifications are cleared on logout.

---

## 11. Session Type Icons

Icons are embedded at compile time from asset files:

| Type | Asset File |
|------|-----------|
| notebook | `assets/session-notebook.jpg` |
| desktop | `assets/session-desktop.png` |
| headless | `assets/session-desktop.png` (same as desktop) |
| carta | `assets/session-carta.png` |
| contributed | `assets/session-contributed.png` |
| firefly | `assets/session-firefly.png` |
| (fallback) | `assets/session-desktop.png` |

Icons are loaded as `gdk_pixbuf::Pixbuf`, scaled to the requested pixel size with bilinear interpolation, converted to a `gdk::Texture`, then wrapped in a `gtk::Image`.

---

## 12. API Endpoints Summary

Base URL: `https://ws-uv.canfar.net` (configurable)

| Purpose | Method | URL | Auth | Request | Response |
|---------|--------|-----|------|---------|----------|
| Login | POST | `https://ws-cadc.canfar.net/ac/login` | None | Form: `username`, `password` | Plain text token |
| Who Am I | GET | `https://ws-cadc.canfar.net/ac/whoami` | Bearer | None | XML `<user>` document |
| List Sessions | GET | `{base}/skaha/v1/session` | Bearer | None | JSON `Vec<SkahaSessionResponse>` |
| Launch Session | POST | `{base}/skaha/v1/session` | Bearer | Form-encoded params | JSON array of session IDs or plain text ID |
| Delete Session | DELETE | `{base}/skaha/v1/session/{id}` | Bearer | None | Empty (200 OK) |
| Renew Session | POST | `{base}/skaha/v1/session/{id}?action=renew` | Bearer | None | Empty (200 OK) |
| Session Events | GET | `{base}/skaha/v1/session/{id}?view=events` | Bearer | None | Plain text |
| Session Logs | GET | `{base}/skaha/v1/session/{id}?view=logs` | Bearer | None | Plain text |
| Platform Stats | GET | `{base}/skaha/v1/session?view=stats` | Bearer | None | JSON `SkahaStatsResponse` (may be in array) |
| List Images | GET | `{base}/skaha/v1/image` | Bearer | None | JSON `Vec<RawImage>` |
| List Repositories | GET | `{base}/skaha/v1/repository` | Bearer | None | JSON `Vec<String>` |
| Resource Context | GET | `{base}/skaha/v1/context` | Bearer | None | JSON `SessionContext` |
| Storage Quota | GET | `{base}/arc/nodes/home/{username}?limit=0` | Bearer | Accept: text/xml | VOSpace XML |

---

## 13. Data Models

### Session (domain model)

```rust
pub struct Session {
    pub id: String,
    pub userid: String,
    pub image: String,               // Full image ID e.g. "images.canfar.net/skaha/notebook:1.0"
    pub session_type: String,         // "notebook", "desktop", "carta", etc.
    pub status: String,               // "Running", "Pending", "Failed", "Terminating"
    pub name: String,                 // User-assigned session name
    pub start_time: String,           // ISO 8601 / RFC 3339
    pub expiry_time: String,          // ISO 8601 / RFC 3339
    pub connect_url: String,          // Full URL to open in browser
    pub requested_ram: String,        // e.g. "8G"
    pub requested_cpu_cores: String,  // e.g. "2"
    pub requested_gpu_cores: String,  // e.g. "0" (defaults to "0" if missing)
    pub ram_in_use: String,           // e.g. "4G"
    pub cpu_cores_in_use: String,     // e.g. "1"
    pub is_fixed_resources: bool,     // true if fixed, false if flex (defaults to true)
}
```

Helper methods:
- `is_running()` -> `status.eq_ignore_ascii_case("running")`
- `is_pending()` -> `status.eq_ignore_ascii_case("pending")`

### SkahaSessionResponse (API response)

```rust
pub struct SkahaSessionResponse {
    pub id: String,                          // Required
    pub userid: Option<String>,
    pub image: Option<String>,
    #[serde(rename = "type")]
    pub session_type: Option<String>,
    pub status: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(rename = "expiryTime")]
    pub expiry_time: Option<String>,
    #[serde(rename = "connectURL")]
    pub connect_url: Option<String>,
    #[serde(rename = "requestedRAM")]
    pub requested_ram: Option<String>,
    #[serde(rename = "requestedCPUCores")]
    pub requested_cpu_cores: Option<String>,
    #[serde(rename = "requestedGPUCores")]
    pub requested_gpu_cores: Option<String>,
    #[serde(rename = "ramInUse")]
    pub ram_in_use: Option<String>,
    #[serde(rename = "cpuCoresInUse")]
    pub cpu_cores_in_use: Option<String>,
    #[serde(rename = "isFixedResources")]
    pub is_fixed_resources: Option<bool>,
}
```

All fields except `id` are optional; defaults applied in `From<SkahaSessionResponse>` for `Session`.

### SessionLaunchParams

```rust
pub struct SessionLaunchParams {
    pub name: String,
    pub image: String,
    pub session_type: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub cmd: Option<String>,
    pub env: Option<String>,
    pub registry_username: Option<String>,  // NOT sent in form body
    pub registry_secret: Option<String>,    // NOT sent in form body
}
```

`to_form_pairs()` produces: `name`, `image`, `type`, `cores`, `ram`, and optionally `gpus` (if > 0), `cmd`, `env`.

Registry credentials are sent as an HTTP header (`x-skaha-registry-auth`), NOT in the form body.

### ParsedImage

```rust
pub struct ParsedImage {
    pub id: String,            // Full image ID "images.canfar.net/skaha/notebook:1.0"
    pub registry: String,      // "images.canfar.net"
    pub project: String,       // "skaha"
    pub name: String,          // "notebook"
    pub version: String,       // "1.0"
    pub types: Vec<String>,    // ["notebook"]
    pub display_name: String,  // "skaha/notebook:1.0"
}
```

### RawImage (API response)

```rust
pub struct RawImage {
    pub id: String,           // Full image ID
    pub types: Vec<String>,   // Session types this image supports
}
```

### SessionContext (API response)

```rust
pub struct SessionContext {
    pub cores: Option<ResourceOption>,
    pub memory_gb: Option<ResourceOption>,  // camelCase: memoryGb
    pub gpus: Option<GpuOption>,
}

pub struct ResourceOption {
    pub default: Option<serde_json::Value>,
    pub options: Option<Vec<serde_json::Value>>,
    pub default_request: Option<String>,       // camelCase: defaultRequest
    pub available_values: Option<Vec<String>>,  // camelCase: availableValues
}

pub struct GpuOption {
    pub options: Option<Vec<serde_json::Value>>,
}
```

Helper methods with fallback chains:
- `core_options()` -> tries `available_values`, then `options`, fallback `[1, 2, 4, 8, 16]`
- `memory_options()` -> tries `available_values`, then `options`, fallback `[1, 2, 4, 8, 16, 32]`
- `gpu_options()` -> tries `options`, fallback `[0, 1, 2]`
- `default_cores()` -> tries `default_request`, then `default`, fallback `2`
- `default_memory()` -> tries `default_request`, then `default`, fallback `8`

### SkahaStatsResponse (Platform Load)

```rust
pub struct SkahaStatsResponse {
    pub instances: Option<InstanceStats>,
    pub cores: Option<CoreStats>,
    pub ram: Option<RamStats>,
}

pub struct InstanceStats {
    pub session: Option<u32>,        // camelCase: session
    pub desktop_app: Option<u32>,    // camelCase: desktopApp
    pub headless: Option<u32>,
    pub total: Option<u32>,
}

pub struct CoreStats {
    pub requested_cpu_cores: Option<serde_json::Value>,  // JSON key: "requestedCPUCores"
    pub cpu_cores_available: Option<serde_json::Value>,   // JSON key: "cpuCoresAvailable"
}

pub struct RamStats {
    pub requested_ram: Option<String>,  // JSON key: "requestedRAM"
    pub ram_available: Option<String>,  // JSON key: "ramAvailable"
}
```

CoreStats values can be numbers or strings. Helper methods: `requested()`, `available()`, `total()` (all -> f64).
RamStats: `requested_gb()`, `available_gb()`, `total_gb()` with suffix parsing (G/Gi/M/Mi/T/Ti).

### StorageQuota

```rust
pub struct StorageQuota {
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub last_update: Option<String>,
}
```

Helper methods:
- `quota_gb()` -> bytes / (1024^3)
- `used_gb()` -> bytes / (1024^3)
- `usage_percent()` -> (used / quota) * 100, returns 0 if quota is 0
- `is_warning()` -> usage_percent > 90.0

### RecentLaunch

```rust
pub struct RecentLaunch {
    pub name: String,
    pub session_type: String,
    pub image: String,           // Full image ID
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub timestamp: String,       // RFC 3339 local time
}
```

Helper methods:
- `display_image()` -> part after last `/`, or full image string
- `type_display()` -> human-readable type name

### SessionTemplate

```rust
pub struct SessionTemplate {
    pub name: String,
    pub description: String,
    pub session_type: String,
    pub image: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub created_at: DateTime<Utc>,
}
```

Stored in `{XDG_DATA_HOME}/Verbinal/templates.json`.

### AuthResult

```rust
pub struct AuthResult {
    pub success: bool,
    pub token: Option<String>,
    pub username: Option<String>,
    pub error: Option<String>,
}
```

### UserInfo

```rust
pub struct UserInfo {
    pub username: Option<String>,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub institute: Option<String>,
    pub internal_id: Option<String>,
}
```

`display_name()` returns `"{first} {last}"` if both non-empty, else `username`, else `"Unknown"`.

Parsed from XML `<user>` document with tags: `<username>`, `<firstName>`, `<lastName>`, `<email>`, `<institute>`, `<internalID>`.

### SessionAction (UI enum)

```rust
pub enum SessionAction {
    Open(String),           // connect URL
    Delete(String, String), // session ID, session name
    Renew(String, String),  // session ID, session name
    Events(String, String), // session ID, session name
}
```

---

## 14. Error Handling

### ApiError Enum

```rust
pub enum ApiError {
    Unauthorized,
    Network(String),
    Server { status: u16, body: String },
    Parse(String),
}
```

The `check_response` function converts HTTP 401/403 to `ApiError::Unauthorized`, other non-success to `ApiError::Server`.

On `Unauthorized` errors, the UI should trigger a re-login flow.

### Error Display

- Unauthorized: "Session expired. Please log in again."
- Network: "Network error: {message}"
- Server: "Server error ({status}): {body}"
- Parse: "Parse error: {message}"

---

## 15. Authentication

### Login Flow

1. User enters username + password in login dialog
2. `POST https://ws-cadc.canfar.net/ac/login` with form body `username={}&password={}`
3. Success (200): response body is a plain text cookie/token string
4. Failure (401): "Invalid username or password"
5. On success:
   - Token stored in system keyring via `keyring` crate (service: "canfar-verbinal", key: "auth-token")
   - Username stored in keyring (key: "username")
   - Token + username stored in `AppServices` (RwLock)
   - `GET https://ws-cadc.canfar.net/ac/whoami` to fetch user info
   - User info stored in `AppServices`

### Auto-Login

On startup:
1. Check keyring for stored token
2. Validate with `GET /ac/whoami`
3. If valid: auto-login, navigate to dashboard
4. If invalid: clear keyring, show "Session expired. Please login."

### Token Usage

All Skaha API calls use `Authorization: Bearer {token}` header via reqwest's `.bearer_auth(token)`.

---

## 16. Configuration

### AppConfig

```rust
pub struct AppConfig {
    pub api_base_url: String,         // Default: "https://ws-uv.canfar.net"
    pub skaha_api_path: String,       // Default: "/skaha/v1"
    pub login_api_path: String,       // Default: "/cred/auth/priv"
    pub ac_api_path: String,          // Default: "/ac"
    pub storage_api_path: String,     // Default: "/arc/nodes/home"
    pub vospace_files_path: String,   // Default: "/arc/files/home"
    pub theme: String,                // Default: "System"
    pub default_session_type: String, // Default: "notebook"
    pub default_cores: u32,           // Default: 2
    pub default_ram: u32,             // Default: 8
}
```

Stored in `{XDG_CONFIG_HOME}/Verbinal/settings.json`.

Note: The login URL is hard-coded to `https://ws-cadc.canfar.net/ac/login` and whoami to `https://ws-cadc.canfar.net/ac/whoami`, NOT derived from `api_base_url`.

---

## 17. Concurrency Architecture

- **Runtime**: Tokio multi-threaded runtime created in `main()`, kept alive for app lifetime
- **Bridge**: `AppServices::spawn()` takes a `Future`, runs it on the Tokio runtime, returns a future that can be awaited on the GLib main loop via a `oneshot` channel
- **UI thread**: All GTK widget manipulation happens on the GLib main thread via `glib::spawn_future_local()`
- **Shared state**: Token, username, user_info are protected by `tokio::sync::RwLock`
- **UI state**: Uses `Rc<RefCell<T>>` for single-threaded UI-side state
- **HTTP client**: Single `reqwest::Client` instance shared across all services
