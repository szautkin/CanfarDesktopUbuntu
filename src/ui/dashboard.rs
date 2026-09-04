use crate::state::AppServices;
use crate::ui::batch_jobs_dialog::show_batch_jobs_dialog;
use crate::ui::batch_jobs_view::BatchJobsView;
use crate::ui::canfar_images::CanfarImagesView;
use crate::ui::delete_dialog::show_delete_dialog;
use crate::ui::launch_dialog::show_launch_dialog;
use crate::ui::launch_form::{LaunchFormView, LaunchTab};
use crate::ui::platform_load::PlatformLoadView;
use crate::ui::recent_launches::RecentLaunchesView;
use crate::ui::session_card::SessionAction;
use crate::ui::session_events_dialog::show_events_dialog;
use crate::ui::session_list::SessionListView;
use crate::ui::space;
use crate::ui::storage_quota::StorageQuotaView;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const MAX_SESSIONS: usize = 3;

pub struct DashboardView {
    container: gtk::Box,
    session_list: Rc<SessionListView>,
    launch_form: Rc<LaunchFormView>,
    platform_load: Rc<PlatformLoadView>,
    storage_quota: Rc<StorageQuotaView>,
    recent_launches: Rc<RecentLaunchesView>,
    batch_jobs: Rc<BatchJobsView>,
    canfar_images: Rc<CanfarImagesView>,
    services: Arc<AppServices>,
    /// The launch modal while it is up, so a successful launch can close it and
    /// a second request can raise it instead of stacking another.
    launch_modal: Rc<RefCell<Option<adw::Window>>>,
}

/// The Portal grid's column count, homogeneous.
const COLUMNS: i32 = 3;

/// One card's cell in the Portal grid: `(column, row, column-span)`.
type Cell = (i32, i32, i32);

/// The cards, in the one order both layouts below are written in.
///
/// The two layouts are the same six widgets in different cells, so they are
/// tables rather than two blocks of `grid.attach` calls: a card added to one
/// arrangement and forgotten in the other is a compile error rather than a card
/// that vanishes when the window is resized.
const CARD_COUNT: usize = 6;

/// Wide: three status cards across, sessions full width, then images 2 : recents 1.
const WIDE: [Cell; CARD_COUNT] = [
    (0, 0, 1),       // platform load
    (1, 0, 1),       // storage
    (2, 0, 1),       // batch jobs
    (0, 1, COLUMNS), // active sessions
    (0, 2, 2),       // CANFAR images
    (2, 2, 1),       // recent launches
];

/// Narrow: one column, same order top to bottom.
const NARROW: [Cell; CARD_COUNT] = [
    (0, 0, COLUMNS),
    (0, 1, COLUMNS),
    (0, 2, COLUMNS),
    (0, 3, COLUMNS),
    (0, 4, COLUMNS),
    (0, 5, COLUMNS),
];

/// Below this width the Portal stacks into a single column.
///
/// It has to sit ABOVE the grid's own minimum, and that minimum is measured,
/// not guessed: `cargo run --features fits --example portal_layout_probe`
/// reports the real cards at
///
/// ```text
/// Batch jobs      minimum  326px  -> needs 326px per column
/// CANFAR images   minimum  581px  -> needs 291px per column
/// => grid minimum      : 978px
/// ```
///
/// This was first set to 720 by reasoning about tile widths. That is *below*
/// 978, so between the two the grid could not shrink and the page scroller —
/// `hscrollbar_policy(Never)`, deliberately, because a sideways-scrolling page
/// is a bug — clipped the right-hand column instead. Batch Jobs and Recent
/// Launches were cut in half at a perfectly ordinary window size, and no amount
/// of resizing restacked them, because the breakpoint that would have was
/// hundreds of pixels further down.
///
/// 1000 leaves headroom over the 978 and matches `panel::COLLAPSE_RIGID_SP`,
/// which is the same judgement about the same kind of content.
///
/// Expressed in `Sp` like the app's other thresholds so it tracks the user's
/// text size rather than raw pixels.
const PORTAL_STACK_SP: f64 = 1000.0;

/// A floor so the grid can actually shrink to the width the breakpoint watches.
///
/// `BreakpointBin` measures its child; without a floor the child's own minimum
/// keeps the bin above the threshold and the breakpoint never fires. Same
/// reason `panel.rs` gives its bin one.
const PORTAL_FLOOR: (i32, i32) = (360, 200);

/// Put each card in its cell.
///
/// One routine for both arrangements, called again whenever the breakpoint
/// flips. Re-attaching requires removing first: a widget already in the grid
/// cannot be attached a second time.
fn place(grid: &gtk::Grid, cards: &[&gtk::Box; CARD_COUNT], cells: &[Cell; CARD_COUNT]) {
    for card in cards.iter() {
        if card.parent().is_some() {
            grid.remove(*card);
        }
    }
    for (card, (col, row, width)) in cards.iter().zip(cells.iter()) {
        grid.attach(*card, *col, *row, *width, 1);
    }
}

impl DashboardView {
    pub fn new(services: Arc<AppServices>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_vexpand(true);

        let grid = gtk::Grid::new();
        grid.set_row_homogeneous(false);
        grid.set_column_homogeneous(true);
        grid.set_vexpand(true);
        grid.set_row_spacing(space::CARD as u32);
        grid.set_column_spacing(space::CARD as u32);

        let session_list = SessionListView::new(services.clone());
        let storage_quota = StorageQuotaView::new(services.clone());
        let batch_jobs = BatchJobsView::new(services.clone());
        let recent_launches = RecentLaunchesView::new(services.clone());
        let platform_load = PlatformLoadView::new(services.clone());
        let canfar_images = CanfarImagesView::new(services.clone());

        // Still owned by the Portal, still wired to everything it always was —
        // the session limit, the launch callback, "Use this image", the image
        // load. Only its PARENT changes: it is presented in a modal instead of
        // occupying two thirds of the page while idle.
        let launch_form = LaunchFormView::new(services.clone(), session_list.sessions_ref());

        let cards: [&gtk::Box; CARD_COUNT] = [
            platform_load.widget(),
            storage_quota.widget(),
            batch_jobs.widget(),
            session_list.widget(),
            canfar_images.widget(),
            recent_launches.widget(),
        ];
        place(&grid, &cards, &WIDE);

        let content = gtk::Box::new(gtk::Orientation::Vertical, space::CARD);
        space::edge_all(&content);
        content.append(&grid);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&content));

        let launch_modal: Rc<RefCell<Option<adw::Window>>> = Rc::new(RefCell::new(None));

        // Restack below the threshold. A `Grid` has no property that reflows,
        // so this reattaches rather than using `add_setter` the way `panel.rs`
        // can for a split view's `collapsed`.
        let bin = adw::BreakpointBin::new();
        bin.set_size_request(PORTAL_FLOOR.0, PORTAL_FLOOR.1);
        bin.set_hexpand(true);
        bin.set_vexpand(true);
        bin.set_child(Some(&scrolled));

        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            PORTAL_STACK_SP,
            adw::LengthUnit::Sp,
        ));
        {
            let grid = grid.clone();
            let owned: Vec<gtk::Box> = cards.iter().map(|c| (*c).clone()).collect();
            breakpoint.connect_apply(move |_| {
                let refs: [&gtk::Box; CARD_COUNT] = std::array::from_fn(|i| &owned[i]);
                place(&grid, &refs, &NARROW);
            });
        }
        {
            let grid = grid.clone();
            let owned: Vec<gtk::Box> = cards.iter().map(|c| (*c).clone()).collect();
            breakpoint.connect_unapply(move |_| {
                let refs: [&gtk::Box; CARD_COUNT] = std::array::from_fn(|i| &owned[i]);
                place(&grid, &refs, &WIDE);
            });
        }
        bin.add_breakpoint(breakpoint);

        container.append(&bin);

        let dashboard = DashboardView {
            container,
            session_list,
            launch_form,
            platform_load,
            storage_quota,
            recent_launches,
            batch_jobs,
            canfar_images,
            services,
            launch_modal,
        };

        dashboard.setup_callbacks();
        dashboard
    }

    fn setup_callbacks(&self) {
        // Session actions (open, delete, renew, events)
        let services = self.services.clone();
        let session_list = self.session_list.clone();
        let launch_form = self.launch_form.clone();
        let recent_launches = self.recent_launches.clone();

        self.session_list.set_on_action(move |action, working| {
            let services = services.clone();
            let session_list = session_list.clone();
            let launch_form = launch_form.clone();
            let recent_launches = recent_launches.clone();

            glib::spawn_future_local(async move {
                // Held for the whole action: the pressed button keeps saying so
                // and the status bar can see the work — including across the
                // delete confirmation, which is itself a wait.
                match action {
                    SessionAction::Open(url) => {
                        // Opening is the browser's job from here; there is
                        // nothing further to wait on.
                        working.finish(&open::that(&url));
                    }
                    SessionAction::Delete(id, name) => {
                        let widget = session_list.widget();
                        if show_delete_dialog(widget, &name).await {
                            let svc = services.clone();
                            let id_c = id.clone();
                            let result = services
                                .spawn(async move {
                                    let token = svc.get_token().await;
                                    let Some(token) = token else {
                                        return Err("No token".to_string());
                                    };
                                    svc.sessions.delete_session(&token, &id_c).await
                                })
                                .await;
                            working.finish(&result);
                            match result {
                                Ok(()) => {
                                    glib::timeout_future_seconds(3).await;
                                    session_list.refresh().await;
                                    update_session_limits(
                                        &session_list,
                                        &launch_form,
                                        &recent_launches,
                                    );
                                }
                                // Reported through the status bar now, rather
                                // than to a stderr nobody is reading.
                                Err(e) => services.toast.toast(crate::tr_fmt!(
                                    "Could not delete {}: {}",
                                    name,
                                    e
                                )),
                            }
                        } else {
                            // Cancelled at the confirmation: not a failure, and
                            // not something to leave recorded as running.
                            working.succeed();
                        }
                    }
                    SessionAction::Renew(id, name) => {
                        // Progress + result dialog (mirrors ShowRenewDialogAsync):
                        // a spinner while renewing, then a success/error message.
                        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                        let spinner = gtk::Spinner::new();
                        spinner.start();
                        let status =
                            gtk::Label::new(Some(&crate::tr_fmt!("Renewing session '{}'…", name)));
                        status.set_wrap(true);
                        status.set_xalign(0.0);
                        content.append(&spinner);
                        content.append(&status);

                        let dialog = adw::MessageDialog::new(
                            crate::ui::dialog::anchor_window(session_list.widget()).as_ref(),
                            Some(crate::tr_en!("Renew Session")),
                            None,
                        );
                        dialog.set_extra_child(Some(&content));
                        dialog.add_response("close", crate::tr_en!("Close"));
                        dialog.set_close_response("close");
                        dialog.present();

                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services
                            .spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else {
                                    return Err("No token".to_string());
                                };
                                svc.sessions.renew_session(&token, &id_c).await
                            })
                            .await;

                        spinner.stop();
                        spinner.set_visible(false);

                        match result {
                            Ok(()) => {
                                dialog.set_heading(Some(crate::tr_en!("Session renewed")));
                                status.set_text(&crate::tr_fmt!(
                                    "'{}' renewed. Its expiry has been extended.",
                                    name
                                ));
                                session_list.refresh().await;
                                update_session_limits(
                                    &session_list,
                                    &launch_form,
                                    &recent_launches,
                                );
                                // Auto-close shortly after success (reference waits 2s).
                                let dialog_ref = dialog.downgrade();
                                glib::spawn_future_local(async move {
                                    glib::timeout_future_seconds(2).await;
                                    if let Some(d) = dialog_ref.upgrade() {
                                        d.close();
                                    }
                                });
                            }
                            Err(e) => {
                                dialog.set_heading(Some(crate::tr_en!("Renew failed")));
                                status.set_text(&crate::tr_fmt!("Renew failed: {}", e));
                            }
                        }
                    }
                    SessionAction::Events(id, name) => {
                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services
                            .spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else {
                                    return ("No auth".to_string(), "No auth".to_string());
                                };
                                let events = svc
                                    .sessions
                                    .get_events(&token, &id_c)
                                    .await
                                    .unwrap_or_else(|e| crate::tr_fmt!("Error: {}", e));
                                let logs = svc
                                    .sessions
                                    .get_logs(&token, &id_c)
                                    .await
                                    .unwrap_or_else(|e| crate::tr_fmt!("Error: {}", e));
                                (events, logs)
                            })
                            .await;
                        let widget = session_list.widget();
                        show_events_dialog(widget, &name, &result.0, &result.1).await;
                    }
                }
            });
        });

        // Session count changes -> update limits
        {
            let launch_form = self.launch_form.clone();
            let recent_launches = self.recent_launches.clone();
            self.session_list.set_on_sessions_changed(move |count| {
                let reached = count >= MAX_SESSIONS;
                launch_form.set_session_limit_reached(reached);
                recent_launches.set_session_limit_reached(reached);
            });
        }

        // The Active Sessions card's own Launch button. The floating button is
        // the other route to the same modal; this one is where someone looking
        // at their sessions expects to find it, and it carries a label.
        {
            let form = self.launch_form.clone();
            let parent = self.container.clone();
            let open = self.launch_modal.clone();
            self.session_list.set_on_launch_requested(move || {
                show_launch_modal(&parent, &form, LaunchTab::Standard, &open);
            });
        }

        // Launch completed -> refresh
        {
            let session_list = self.session_list.clone();
            let recent_launches = self.recent_launches.clone();
            let batch_jobs = self.batch_jobs.clone();
            let launch_modal = self.launch_modal.clone();
            self.launch_form.set_on_launched(move || {
                // The session started; the form has done its job. Leaving the
                // modal up hides the session list the user now wants to see,
                // and invites a second launch of the same thing.
                // Taken into a local FIRST. `if let Some(w) = cell.borrow_mut()
                // .take()` holds the RefMut for the whole body, and closing the
                // window runs the close handler, which takes the same cell —
                // "already borrowed", inside a GTK signal trampoline that
                // cannot unwind, so the process aborts rather than panicking.
                let open = launch_modal.borrow_mut().take();
                if let Some(window) = open {
                    window.close();
                }
                let session_list = session_list.clone();
                let recent_launches = recent_launches.clone();
                let batch_jobs = batch_jobs.clone();
                glib::spawn_future_local(async move {
                    glib::timeout_future_seconds(2).await;
                    session_list.refresh().await;
                    recent_launches.refresh();
                    // The Headless tab launches through this same callback, and
                    // the Batch Jobs card is what notifies when a job ends. It
                    // was never told, so a job submitted while the card had
                    // backed off to the idle cadence went unseen until the next
                    // scheduled poll — the counts stale for that long, and the
                    // clock on its completion notification starting that late.
                    batch_jobs.refresh().await;
                });
            });
        }

        // Relaunch from recent
        {
            let services = self.services.clone();
            let session_list = self.session_list.clone();
            let recent_launches_ref = self.recent_launches.clone();
            self.recent_launches
                .set_on_relaunch(move |launch, working| {
                    let services = services.clone();
                    let session_list = session_list.clone();
                    let recent_launches_ref = recent_launches_ref.clone();

                    glib::spawn_future_local(async move {
                        // Faithful relaunch (mirrors RelaunchAsync): reuse the recorded
                        // session name and reproduce the saved configuration. A
                        // flexible record zeroes cores/ram/gpus (platform-managed);
                        // a headless record replays cmd/args/replicas and routes
                        // through the headless launch (its session_type drives the
                        // batch-job form fields), never the interactive path.
                        let name = launch.name.clone();
                        let params = launch.to_launch_params(name.clone());

                        let svc = services.clone();
                        let params_clone = params.clone();
                        let result = services
                            .spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else {
                                    return Err("No token".to_string());
                                };
                                svc.sessions.launch_session(&token, &params_clone).await
                            })
                            .await;

                        // Button restored and the task recorded, in one place.
                        working.finish(&result);

                        let image_display = match params.image.rsplit_once('/') {
                            Some((_, tail)) => tail.to_string(),
                            None => params.image.clone(),
                        };

                        show_launch_dialog(
                            recent_launches_ref.widget(),
                            &name,
                            &image_display,
                            &params.session_type,
                            params.cores,
                            params.ram,
                            params.gpus,
                            result.clone(),
                        )
                        .await;

                        if result.is_ok() {
                            glib::timeout_future_seconds(2).await;
                            session_list.refresh().await;
                            recent_launches_ref.refresh();
                        }
                    });
                });
        }

        // Launch from template
        // Batch Jobs — state-click opens the drill-down dialog
        {
            let services = self.services.clone();
            let batch_jobs = self.batch_jobs.clone();
            self.batch_jobs.set_on_state_click(move |state, jobs| {
                let services = services.clone();
                let parent = batch_jobs.widget().clone();
                glib::spawn_future_local(async move {
                    show_batch_jobs_dialog(&parent, services, jobs, state).await;
                });
            });
        }

        // Canfar Images — "Use this image" selects it in the launch form
        // (mirrors DashboardPage.OnUseImageRequested → SelectImageById).
        //
        // Straight to the form, which this same dashboard owns and shows two
        // cards away. It used to activate a `use-launch-image` app action that
        // NOBODY REGISTERED, so both buttons that reach here — the one on each
        // image row, and the one in the find-by-package dialog when that dialog
        // is opened from this card — did nothing whatsoever. GIO logs an
        // unregistered activation and carries on, so there was not even an
        // error to notice.
        {
            let launch_form = self.launch_form.clone();
            let services = self.services.clone();
            // The form is no longer sitting on the page, so selecting an image
            // in it would be invisible — this has to OPEN it too. Which tab it
            // lands on is decided by `select_image_by_id`, so the modal is
            // presented after it has chosen.
            let parent = self.container.clone();
            let open = self.launch_modal.clone();
            self.canfar_images.set_on_use_image(move |image_id| {
                let placed = launch_form.select_image_by_id(&image_id);
                show_launch_modal(&parent, &launch_form, launch_form.current_tab(), &open);
                if !placed {
                    // It went to the Advanced tab's custom-image field instead.
                    // Say so: the Standard tab is where the eye goes, and a
                    // selection that landed elsewhere looks like nothing
                    // happened.
                    services.toast.toast(crate::tr_fmt!(
                        "{} is not offered for a session type — put it in the Advanced tab",
                        image_id
                    ));
                }
            });
        }
    }

    pub async fn load_data(&self) {
        // Warm the manifest cache OFF the GTK thread before anything draws
        // from it.
        //
        // `JsonManifestStore` hydrates lazily: the first read walks every
        // cached outcome on disk and parses it. Measured at 13.6–20.8 ms for
        // 266 files (`cargo run --release --features fits --example
        // blocking_probe`) — more than a frame — and it grows with the
        // catalogue. Whoever read it first paid that, on the main loop, mid
        // draw. Now nobody does.
        {
            let store = Arc::clone(&self.services.image_manifests);
            let _ = self
                .services
                .spawn(async move {
                    store.row_summaries();
                })
                .await;
        }

        self.session_list.refresh().await;
        self.storage_quota.refresh().await;
        self.platform_load.refresh().await;
        self.launch_form.load_images().await;
        self.batch_jobs.refresh().await;
        self.canfar_images.refresh().await;

        self.recent_launches.refresh();

        update_session_limits(&self.session_list, &self.launch_form, &self.recent_launches);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

/// Present the launch form in a modal, opened on `tab`.
///
/// The form is a long-lived widget the dashboard owns, not something built per
/// dialog: it holds the loaded image catalogue, the session-limit state and the
/// launch callback, and rebuilding it per opening would refetch all of that.
/// So it is LENT to the dialog and taken back when the dialog closes — without
/// that, the next opening tries to parent an already-parented widget and GTK
/// refuses.
fn show_launch_modal(
    parent: &impl IsA<gtk::Widget>,
    form: &Rc<LaunchFormView>,
    tab: LaunchTab,
    open: &Rc<RefCell<Option<adw::Window>>>,
) {
    // Already up — just switch tabs rather than stacking a second dialog.
    // Cloned out of the cell before use, for the same reason the close path
    // below takes rather than borrows: `present` can emit signals that reach
    // handlers wanting this cell.
    let existing = open.borrow().clone();
    if let Some(window) = existing {
        form.show_tab(tab);
        window.present();
        return;
    }

    // Tall enough for the Advanced tab, which is twice the length of Standard,
    // without the Standard tab opening as a band of empty space. The cap is a
    // share of the window rather than a number, because the three tabs differ
    // enough that any fixed height is wrong for two of them.
    let dialog = crate::ui::dialog::Dialog::new(
        crate::tr_en!("Launch Session"),
        crate::ui::fit::DETAIL,
        crate::ui::dialog::viewport_share(parent, LAUNCH_MODAL_SHARE, LAUNCH_MODAL_HEIGHT),
    );
    form.show_tab(tab);

    // The dialog's header bar already says "Launch Session"; the card's own
    // heading would say it a second time, ten pixels below — and its frame
    // would draw a second box just inside the dialog's own edge.
    form.set_header_visible(false);
    form.set_framed(false);

    // Take the form off whatever holds it. The close handler does this on the
    // normal path, but re-entry would otherwise parent a parented widget.
    if let Some(old) = form.widget().parent().and_downcast::<gtk::Box>() {
        old.remove(form.widget());
    }
    dialog.content().append(form.widget());

    // The launch buttons go in the dialog's own action bar, which is pinned
    // BELOW the scroller. Left inside the form they scroll with it, and the
    // form is taller than the dialog — so the one button the dialog exists for
    // sat below the fold.
    if let Some(row) = form.action_row().parent().and_downcast::<gtk::Box>() {
        row.remove(form.action_row());
    }
    dialog.add_action(form.action_row());

    {
        let form = form.clone();
        let content = dialog.content().clone();
        let open = open.clone();
        dialog.window.connect_close_request(move |_| {
            if form.action_row().parent().is_some() {
                form.action_row().unparent();
            }
            form.restore_action_row();
            if form.widget().parent().is_some() {
                content.remove(form.widget());
            }
            form.set_header_visible(true);
            form.set_framed(true);
            // Back to the form for the next opening: a modal reopened on the
            // confirmation of the last launch would be showing an answer to a
            // question nobody has asked yet.
            form.clear_result();
            *open.borrow_mut() = None;
            gtk::glib::Propagation::Proceed
        });
    }

    *open.borrow_mut() = Some(dialog.window.clone());
    dialog.present(parent);
}

/// Height CAP for the launch modal — see `ui::dialog::Dialog::new`.
///
/// The Advanced tab carries two preferences groups and the Headless tab a form
/// plus a resource selector, so the cap is generous; anything taller scrolls
/// rather than pushing the window off the screen.
const LAUNCH_MODAL_HEIGHT: i32 = 640;

/// How much of the window the launch modal may fill before it scrolls.
const LAUNCH_MODAL_SHARE: f64 = 0.8;

fn update_session_limits(
    session_list: &Rc<SessionListView>,
    launch_form: &Rc<LaunchFormView>,
    recent_launches: &Rc<RecentLaunchesView>,
) {
    let count = session_list.session_count();
    let reached = count >= MAX_SESSIONS;
    launch_form.set_session_limit_reached(reached);
    recent_launches.set_session_limit_reached(reached);
}

#[cfg(test)]
mod use_image_tests {
    use super::*;

    const SOURCE: &str = include_str!("dashboard.rs");

    #[test]
    fn the_portal_rows_are_the_shape_they_are_meant_to_be() {
        // The arrangement, asserted as the arrangement rather than as a count
        // of `grid.attach` calls: three status cards across the top, the
        // session list across the whole width, then images to recents 2 : 1.
        let (top, rest) = WIDE.split_at(3);
        for (i, (col, row, width)) in top.iter().enumerate() {
            assert_eq!(*row, 0, "status card {i} left the top row");
            assert_eq!(*width, 1, "status card {i} is no longer a third");
            assert_eq!(*col, i as i32, "the status cards are out of order");
        }
        assert_eq!(rest[0], (0, 1, COLUMNS), "sessions lost its full width");
        assert_eq!(rest[1].2, 2, "CANFAR images is no longer two thirds");
        assert_eq!(rest[2].2, 1, "recent launches is no longer one third");
        assert_eq!(
            rest[1].2 + rest[2].2,
            COLUMNS,
            "the images/recents row does not fill the width"
        );
    }

    #[test]
    fn nothing_the_portal_loads_hydrates_the_cache_on_the_gtk_thread() {
        // The manifest store hydrates on first read — every cached outcome
        // walked and parsed. Measured at 13.6–20.8 ms for 266 files, which is
        // more than a frame, and it grows with the catalogue. Whichever widget
        // read it first paid that on the main loop.
        //
        // `load_data` must warm it through `services.spawn` (the Tokio bridge)
        // BEFORE the cards that read it refresh.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        let at = code
            .find("pub async fn load_data")
            .expect("load_data is gone");
        let body = &code[at..];
        let warm = body
            .find("row_summaries()")
            .expect("load_data no longer warms the manifest cache");
        let images = body
            .find("canfar_images.refresh()")
            .expect("the images card no longer refreshes");
        assert!(
            warm < images,
            "the images card reads the manifest cache before it has been warmed, \
             so the hydration happens on the GTK thread"
        );
        assert!(
            body[..warm].contains("services\n                .spawn(")
                || body[..warm].contains("services.spawn("),
            "the warm-up is not going through the Tokio bridge, so it is still \
             on the GTK thread"
        );
    }

    #[test]
    fn the_portal_stacks_before_it_starts_clipping() {
        /// The grid's measured minimum, from `examples/portal_layout_probe.rs`.
        ///
        /// Declared HERE, not beside `PORTAL_STACK_SP`: `testing::code` cuts a
        /// source file at its first `#[cfg(test)]`, so a test-only item near the
        /// top of the file blinds every source-scanning guard below it. Four of
        /// them went green-to-red on nothing but that.
        ///
        /// Re-run the probe and update this when a card's minimum changes.
        const PORTAL_GRID_MIN_PX: f64 = 978.0;

        // The failure this prevents is a threshold set below the grid's own
        // minimum: the grid cannot shrink past its minimum, the page scroller
        // refuses to scroll sideways, and the right-hand column is cut off at a
        // window size where nothing restacks. That shipped once, at 720 against
        // a measured 978.
        // `black_box` so this is a comparison and not a constant clippy folds.
        let threshold = std::hint::black_box(PORTAL_STACK_SP);
        assert!(
            threshold > PORTAL_GRID_MIN_PX,
            "the Portal stacks at {PORTAL_STACK_SP}px but cannot render below              {PORTAL_GRID_MIN_PX}px, so between the two it clips instead"
        );
    }

    #[test]
    fn every_card_has_a_cell_in_both_arrangements() {
        // The failure this prevents is a card added to one layout and forgotten
        // in the other — visible at one window size and gone at the other,
        // which reads as a rendering bug rather than a missing line.
        assert_eq!(WIDE.len(), CARD_COUNT);
        assert_eq!(NARROW.len(), CARD_COUNT);
        for (i, (col, _, width)) in NARROW.iter().enumerate() {
            assert_eq!(*col, 0, "stacked card {i} is not in the single column");
            assert_eq!(*width, COLUMNS, "stacked card {i} does not span the width");
        }
        let rows: Vec<i32> = NARROW.iter().map(|(_, row, _)| *row).collect();
        let mut sorted = rows.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            rows, sorted,
            "stacked cards share a row or are out of order"
        );
    }

    #[test]
    fn the_launch_form_is_reached_through_the_modal_not_the_grid() {
        // It used to occupy two thirds of the second row permanently. If it is
        // attached to the grid again it will be parented twice — once by the
        // grid and once by the dialog — and GTK will refuse the second.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            !code.contains("launch_form.widget()"),
            "the launch form is back in the Portal grid; it is the modal's now"
        );
        assert!(
            code.contains("show_launch_modal("),
            "nothing opens the launch modal"
        );
    }

    #[test]
    fn no_borrow_is_held_across_closing_the_modal() {
        // `if let Some(w) = cell.borrow_mut().take() { w.close(); }` keeps the
        // RefMut alive for the whole body. Closing the window runs the close
        // handler, which wants the same cell, and "already borrowed" inside a
        // GTK signal trampoline is a panic that cannot unwind — the process
        // aborts with a core dump rather than reporting anything.
        //
        // The cell must be emptied into a local first, so the borrow ends
        // before anything re-entrant runs.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        for forbidden in [
            "if let Some(window) = launch_modal.borrow_mut().take()",
            "if let Some(window) = open.borrow_mut().take()",
            "if let Some(window) = open.borrow().as_ref()",
        ] {
            assert!(
                !code.contains(forbidden),
                "a borrow of the modal cell is held across a call that re-enters \
                 it: {forbidden}"
            );
        }
    }

    #[test]
    fn the_modal_gives_the_form_back_when_it_closes() {
        // The form outlives the dialog. Without the hand-back the next opening
        // parents an already-parented widget, GTK refuses, and the modal comes
        // up empty — once, silently, on the second use.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        let at = code
            .find("fn show_launch_modal")
            .expect("show_launch_modal is gone");
        let body = &code[at..];
        let end = body.find("\n}\n").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("connect_close_request"),
            "the modal never takes the form back"
        );
        assert!(
            body.contains("content.remove(form.widget())"),
            "the form is not removed from the dialog on close"
        );
    }

    #[test]
    fn use_this_image_reaches_the_launch_form() {
        // It used to activate a `use-launch-image` app action that nobody
        // registered, so every button routing here — the one on each image row,
        // and the one in the find-by-package dialog when that dialog is opened
        // from this card — did nothing at all. The dashboard owns the launch
        // form two cards away; there was never a reason to go out through GIO
        // and back.
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("set_on_use_image")
            .expect("nothing handles Use this image");
        let handler = &code[at..(at + 900).min(code.len())];
        assert!(
            handler.contains("select_image_by_id"),
            "Use this image no longer selects anything in the launch form"
        );
        assert!(
            !handler.contains("activate_action"),
            "the request is going back out through an app action"
        );
    }
}
