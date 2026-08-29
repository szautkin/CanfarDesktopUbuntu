pub mod agent_badge;
pub mod agent_proposals_dialog;
pub mod ai_connect_wizard;
pub mod ai_guide_page;
pub mod annotations_panel;
pub mod batch_jobs_dialog;
pub mod batch_jobs_view;
pub mod canfar_images;
pub mod card;
pub mod coord_chip;
pub mod cube_export;
pub mod cube_slice_view;
pub mod cube_tab_host;
pub mod cube_viewer;
pub mod cube_volume_gl;
pub mod dashboard;
pub mod datalink_file_dialog;
pub mod delete_dialog;
pub mod dialog;
pub mod failure_detail;
pub mod file_panel;
pub mod fit;
pub mod fits_canvas;
pub mod fits_coords_panel;
pub mod fits_header_panel;
pub mod fits_tab;
pub mod fits_viewer;
pub mod image_discovery_dialog;
pub mod item_list_section;
pub mod launch_dialog;
pub mod launch_form;
pub mod login_dialog;
pub mod main_window;
pub mod metric_bar;
pub mod notebook_cell;
pub mod notebook_host;
pub mod notebook_page;
pub mod observation_detail_page;
pub mod platform_load;
pub mod recent_launches;
pub mod rename_dialog;
pub mod research_page;
pub mod resource_selector;
pub mod saved_query_dialog;
pub mod search_page;
pub mod session_card;
pub mod session_events_dialog;
pub mod session_icon;
pub mod session_list;
pub mod settings_page;
pub mod share_dialog;
pub mod space;
pub mod storage_quota;
pub mod text_viewer_dialog;
pub mod vospace_browser;
pub mod workflows_page;

pub use main_window::build_main_window;

use std::cell::RefCell;
use std::rc::Rc;

/// A late-bound, optional UI callback owned by one widget.
///
/// Widgets are constructed before their host knows what to do with their events,
/// so the host installs the handler afterwards — hence `RefCell<Option<_>>`.
/// `Rc` (not `Box`) so a handler can be cloned out and invoked without holding
/// the borrow across the call, which would panic if the handler re-entered the
/// widget.
pub type CallbackSlot<F> = RefCell<Option<Rc<F>>>;

/// A [`CallbackSlot`] shared across clones of a widget handle — the same slot
/// seen by every closure that captured the widget.
pub type SharedCallbackSlot<F> = Rc<CallbackSlot<F>>;
pub mod viewer_shell;
