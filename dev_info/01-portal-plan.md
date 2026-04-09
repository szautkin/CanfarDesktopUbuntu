# Portal Module -- Implementation Plan

## Current State

The Portal module is **largely implemented** and functional. The dashboard layout (2x2 grid), session list with auto-poll, launch form with cascading dropdowns, platform load, storage quota, and recent launches all work. The following services exist but are **not wired into the Portal UI**:

- `NotificationService` at `src/services/notification_service.rs` -- fully implemented with `notify_session_ready()`, `notify_session_failed()`, `notify_session_expiring()`, and dedup via `HashSet<String>`. Never called from session polling.
- `TemplateService` at `src/services/template_service.rs` -- fully implemented with `load()`, `save()`, `add()`, `remove()`. Never wired into the dashboard.

Both are registered in `AppServices` at `src/state.rs` (lines 19-20).

---

## Gap 1: Session Card Does Not Display ram_in_use / cpu_cores_in_use

**Spec reference**: Section 2 -- "CPU: 2 RAM: 8G" row in card; the `Session` model (Section 13) has `ram_in_use` and `cpu_cores_in_use` fields.

**Current code**: `src/ui/session_card.rs` lines 104-125 only display `requested_cpu_cores` and `requested_ram`. The fields `ram_in_use` and `cpu_cores_in_use` exist on the `Session` model but are never read in the card.

### Changes

**File**: `src/ui/session_card.rs`

After the existing resource row (line 125, after the `if !session.is_fixed_resources` block), add a secondary usage row:

```rust
// --- Add after line 125 (after the FLEX badge block) ---

// In-use resources (only show if session is running and values differ from "0"/empty)
if session.is_running()
    && (!session.cpu_cores_in_use.is_empty() || !session.ram_in_use.is_empty())
{
    let usage_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);

    let usage_prefix = gtk::Label::new(Some("In use:"));
    usage_prefix.add_css_class("caption");
    usage_prefix.add_css_class("dim-label");
    usage_box.append(&usage_prefix);

    if !session.cpu_cores_in_use.is_empty() && session.cpu_cores_in_use != "0" {
        let cpu_use = gtk::Label::new(Some(&format!("CPU: {}", session.cpu_cores_in_use)));
        cpu_use.add_css_class("caption");
        usage_box.append(&cpu_use);
    }

    if !session.ram_in_use.is_empty() && session.ram_in_use != "0" {
        let ram_use = gtk::Label::new(Some(&format!("RAM: {}", session.ram_in_use)));
        ram_use.add_css_class("caption");
        usage_box.append(&ram_use);
    }

    inner.append(&usage_box);
}
```

**No model changes needed** -- `Session` already has these fields.

---

## Gap 2: No Batch Jobs / Headless Filter Widget

**Spec reference**: Section 2 -- "Headless sessions are NOT filtered out... All sessions returned by the API are displayed."

The spec says no filtering is applied, so headless sessions are already shown. However, the Windows version has a filter dropdown that lets users filter by session type (All / Interactive / Headless). This is missing.

### Changes

**File**: `src/ui/session_list.rs`

**Step 2a**: Add a `filter_combo` field to `SessionListView`.

Insert into the struct definition (after line 23, `countdown_label`):

```rust
filter_combo: gtk::DropDown,
filtered_sessions: Rc<RefCell<Vec<Session>>>,
```

**Step 2b**: Build the filter dropdown in `new()`, after the header construction (around line 44).

```rust
let filter_list = gtk::StringList::new(&["All", "Interactive", "Headless"]);
let filter_combo = gtk::DropDown::new(Some(filter_list), gtk::Expression::NONE);
filter_combo.set_selected(0);
// Insert into header before the refresh button
header.insert_child_after(&filter_combo, Some(&count_label));
```

**Step 2c**: Wire `filter_combo.connect_selected_notify` to call a new `apply_filter()` method.

**Step 2d**: Add `apply_filter(&self)` method:

```rust
fn apply_filter(&self) {
    let filter_idx = self.filter_combo.selected();
    let sessions = self.sessions.borrow();
    let filtered: Vec<Session> = sessions.iter().filter(|s| {
        match filter_idx {
            1 => !s.session_type.eq_ignore_ascii_case("headless"), // Interactive only
            2 => s.session_type.eq_ignore_ascii_case("headless"),  // Headless only
            _ => true, // All
        }
    }).cloned().collect();

    // Rebuild cards from filtered list
    while let Some(child) = self.cards_box.first_child() {
        self.cards_box.remove(&child);
    }
    for session in &filtered {
        let card = SessionCard::new(session, self.on_action.clone());
        self.cards_box.append(card.widget());
    }
    self.count_label.set_text(&format!(
        "{} session{}", filtered.len(), if filtered.len() == 1 { "" } else { "s" }
    ));
    self.empty_label.set_visible(filtered.is_empty());
}
```

**Step 2e**: Call `apply_filter()` at the end of `update_sessions()` (line 159) instead of rebuilding cards inline. Refactor so `update_sessions` stores all sessions, then calls `apply_filter()` for the display step.

---

## Gap 3: default_session_type Not Pre-Selected in Launch Form Dropdown

**Spec reference**: Section 16 -- `AppConfig.default_session_type` defaults to `"notebook"`.

**Current code**: `src/ui/launch_form.rs` line 82 -- `type_combo` is created with the list but the initial selection is always index 0 (implicitly). The config's `default_session_type` is never read.

### Changes

**File**: `src/ui/launch_form.rs`

After the `type_combo` is created (line 82), add:

```rust
// Pre-select default session type from config
let default_type = services.endpoints.config().default_session_type.clone();
let type_names = ["notebook", "desktop", "carta", "contributed", "firefly"];
let default_idx = type_names.iter().position(|t| *t == default_type).unwrap_or(0);
type_combo.set_selected(default_idx as u32);
```

This should be placed around line 83, after `let type_combo = ...` and before it's added to the form group.

---

## Gap 4: NotificationService Exists But Is Never Called During Poll

**Spec reference**: Section 10 -- Notification types: Session Ready, Session Failed, Session Expiring.

**Current code**: `src/ui/session_list.rs` -- the auto-poll loop in `update_sessions()` (lines 166-233) rebuilds cards but never calls `NotificationService`. The service is available at `self.services.notifications`.

### Changes

**File**: `src/ui/session_list.rs`

**Step 4a**: Add a `previous_sessions` field to `SessionListView` to track state transitions:

```rust
previous_sessions: Rc<RefCell<Vec<Session>>>,
```

Initialize as empty `Vec::new()` in `new()`.

**Step 4b**: Add a helper method to detect state changes and fire notifications:

```rust
fn check_notifications(&self, old: &[Session], new: &[Session]) {
    // Get the GTK application for sending notifications
    let Some(app) = self.container.root()
        .and_then(|w| w.downcast_ref::<gtk::Window>().map(|w| w.application()))
        .flatten()
        .and_then(|a| a.downcast::<gtk::Application>().ok())
    else { return; };

    let gio_app: &gtk4::gio::Application = app.upcast_ref();

    for session in new {
        let was_pending = old.iter().any(|s| s.id == session.id && s.is_pending());

        // Session became Running (was Pending before)
        if session.is_running() && was_pending {
            self.services.notifications.notify_session_ready(
                gio_app, &session.id, &session.name, &session.session_type,
            );
        }

        // Session became Failed
        if session.status.eq_ignore_ascii_case("failed") {
            let was_failed = old.iter().any(|s| s.id == session.id && s.status.eq_ignore_ascii_case("failed"));
            if !was_failed {
                self.services.notifications.notify_session_failed(
                    gio_app, &session.id, &session.name,
                );
            }
        }

        // Session expiring within 1 hour
        if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&session.expiry_time) {
            let now = chrono::Utc::now();
            let remaining = expiry.signed_duration_since(now);
            if remaining.num_hours() < 1 && remaining.num_seconds() > 0 {
                self.services.notifications.notify_session_expiring(
                    gio_app, &session.id, &session.name,
                );
            }
        }
    }
}
```

**Step 4c**: Call `check_notifications()` in `update_sessions()`, right before `*self.sessions.borrow_mut() = sessions;` (line 159):

```rust
let old_sessions = self.sessions.borrow().clone();
self.check_notifications(&old_sessions, &sessions);
```

**Step 4d**: Do the same in the auto-poll loop body (around line 222), before `*sessions_ref.borrow_mut() = new_sessions;`:

```rust
// Need to pass NotificationService reference into the closure
let old = sessions_ref.borrow().clone();
// call check_notifications with old and new_sessions
```

This requires restructuring the auto-poll closure to capture `self` or pass the notification service. The cleanest approach is to extract `check_notifications` as a free function that takes the required arguments.

---

## Gap 5: TemplateManager Exists But Is Not Wired Into Dashboard

**Spec reference**: Section 13 -- `SessionTemplate` model exists. `TemplateService` has `load()`, `save()`, `add()`, `remove()`.

**Current code**: The `TemplateService` is registered in `AppServices` but never used by any UI component.

### Changes

**File**: `src/ui/dashboard.rs`

**Step 5a**: Add a "Save as Template" button to the launch form. This is most naturally placed in `src/ui/launch_form.rs`.

**File**: `src/ui/launch_form.rs`

After the launch button (line 208), add a "Save Template" button:

```rust
let save_template_btn = gtk::Button::with_label("Save Template");
save_template_btn.set_tooltip_text(Some("Save current configuration as a reusable template"));
bottom.append(&save_template_btn);
```

Wire the click handler to build a `SessionTemplate` from the current form state and call `services.templates.add(template)`.

**Step 5b**: Add a templates section to `RecentLaunchesView` or create a new `TemplatesView` widget.

**File**: `src/ui/recent_launches.rs`

Add a "Templates" section below the recent launches list:

```rust
// After the recent launches list_box, add:
let templates_title = gtk::Label::new(Some("Templates"));
templates_title.add_css_class("title-4");
templates_title.set_margin_top(12);
// ... templates_list_box with adw::ActionRow items
// Each row has: name, description, relaunch button, delete button
```

**Step 5c**: Wire template selection to populate the launch form. Add a callback `on_template_selected` that sets the launch form's type, image, cores, ram, gpus from the template.

**Dependency**: This gap is lower priority than gaps 1-4 since the core template service works; it just lacks UI exposure.

---

## Gap 6: No Expiry Time Warning Highlighting on Session Cards

**Spec reference**: Section 2 -- The expiry time should be visually highlighted when a session is close to expiring.

**Current code**: `src/ui/session_card.rs` lines 89-98 display the expiry time as a plain label with no conditional styling.

### Changes

**File**: `src/ui/session_card.rs`

Replace the expiry label construction (lines 91-97) with conditional styling:

```rust
if !session.expiry_time.is_empty() {
    let expiry_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let expiry_icon = gtk::Image::from_icon_name("alarm-symbolic");
    expiry_icon.set_pixel_size(12);
    expiry_box.append(&expiry_icon);

    let expiry_text = gtk::Label::new(Some(&format_time(&session.expiry_time)));
    expiry_text.add_css_class("caption");

    // Highlight if expiring within 1 hour
    if let Ok(expiry_dt) = chrono::DateTime::parse_from_rfc3339(&session.expiry_time) {
        let now = chrono::Utc::now();
        let remaining = expiry_dt.signed_duration_since(now);
        if remaining.num_hours() < 1 && remaining.num_seconds() > 0 {
            expiry_text.add_css_class("error");  // Red text
            expiry_icon.add_css_class("error");
        } else if remaining.num_hours() < 24 {
            expiry_text.add_css_class("warning"); // Amber text
        }
    }

    expiry_box.append(&expiry_text);
    times_box.append(&expiry_box);
}
```

**File**: `src/style.css` (or wherever CSS is defined)

Ensure the `error` and `warning` CSS classes apply appropriate text colors. These are typically provided by libadwaita's default stylesheet but verify:

```css
.error { color: @error_color; }
.warning { color: @warning_color; }
```

---

## Implementation Order

| Priority | Gap | File(s) | Effort | Dependencies |
|----------|-----|---------|--------|-------------|
| 1 | Gap 6: Expiry warning highlighting | `src/ui/session_card.rs` | Small (15 min) | None |
| 2 | Gap 1: ram_in_use / cpu_cores_in_use display | `src/ui/session_card.rs` | Small (15 min) | None |
| 3 | Gap 3: default_session_type pre-selection | `src/ui/launch_form.rs` | Small (10 min) | None |
| 4 | Gap 4: Wire NotificationService to poll | `src/ui/session_list.rs` | Medium (45 min) | None |
| 5 | Gap 2: Session type filter widget | `src/ui/session_list.rs` | Medium (30 min) | None |
| 6 | Gap 5: Wire TemplateService to dashboard | `src/ui/launch_form.rs`, `src/ui/recent_launches.rs` | Medium (1 hr) | None |

All gaps are independent and can be implemented in any order. Gaps 1, 3, and 6 are trivial. Gap 4 is the highest-value improvement (users get desktop notifications). Gap 5 is a nice-to-have UX feature.
