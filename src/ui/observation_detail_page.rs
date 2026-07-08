//! Full-width CAOM2 observation detail viewer.
//!
//! Mirrors `Views/ObservationDetailPage.xaml.cs` from CanfarDesktop: a single
//! `gtk::Stack` switches between Loading / AuthRequired / NotFound / ServerError
//! and a Success view that shows a header (collection + observationID + type/intent
//! chips) and a 5-tab `gtk::Notebook`:
//!
//!   * Overview   — identity / target / proposal / telescope+instrument / environment
//!   * Coverage   — per-plane spatial footprint (drawn polygon), spectral, temporal
//!   * Files      — per-plane artifacts with a best-effort Download button
//!   * Provenance — algorithm / sequence / plane products
//!   * Raw        — a monospace dump of the key CAOM2 fields
//!
//! The page fetches metadata off the GLib main thread via a `CAOM2Service` created
//! on demand (`services.spawn` bridges tokio → glib).

use crate::models::caom2::{CAOM2Observation, Caom2Artifact};
use crate::models::search_result::DataLinkResult;
use crate::services::caom2_service::{CAOM2Service, Caom2Status};
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Em dash used for "no value", matching `Caom2Format.Dash`.
const DASH: &str = "—";

// ---------------------------------------------------------------------------
// ObservationDetailPage
// ---------------------------------------------------------------------------

pub struct ObservationDetailPage {
    pub widget: gtk::Box,
    services: Arc<AppServices>,
    /// State switcher: loading / auth / notfound / error / success.
    stack: gtk::Stack,
    /// Container for the Success view — cleared and rebuilt on every load.
    success_container: gtk::Box,
    /// Not-found status page (description updated per publisher id).
    notfound_page: adw::StatusPage,
    /// Error status page (description holds the service error message).
    error_page: adw::StatusPage,
    /// Publisher id of the observation currently being shown. Used by Retry and
    /// to discard a stale fetch when the user navigates away mid-load.
    current_publisher_id: RefCell<String>,
}

impl ObservationDetailPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(150);

        // ── Loading ────────────────────────────────────────────────────
        let loading = gtk::Box::new(gtk::Orientation::Vertical, 12);
        loading.set_halign(gtk::Align::Center);
        loading.set_valign(gtk::Align::Center);
        loading.set_vexpand(true);
        loading.set_hexpand(true);
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(36, 36);
        spinner.start();
        loading.append(&spinner);
        let loading_label = gtk::Label::new(Some(crate::tr_en!("Loading observation…")));
        loading_label.add_css_class("dim-label");
        loading.append(&loading_label);
        stack.add_named(&loading, Some("loading"));

        // ── Auth required ──────────────────────────────────────────────
        let auth_page = adw::StatusPage::new();
        auth_page.set_icon_name(Some("channel-secure-symbolic"));
        auth_page.set_title(crate::tr_en!("Sign-in required"));
        auth_page.set_description(Some(crate::tr_en!(
            "This observation's metadata is proprietary. Sign in with the account button, then retry."
        )));
        let auth_retry = pill_button(crate::tr_en!("Retry"));
        auth_page.set_child(Some(&auth_retry));
        stack.add_named(&auth_page, Some("auth"));

        // ── Not found ──────────────────────────────────────────────────
        let notfound_page = adw::StatusPage::new();
        notfound_page.set_icon_name(Some("edit-find-symbolic"));
        notfound_page.set_title(crate::tr_en!("Observation not found"));
        stack.add_named(&notfound_page, Some("notfound"));

        // ── Server / parse error ───────────────────────────────────────
        let error_page = adw::StatusPage::new();
        error_page.set_icon_name(Some("network-error-symbolic"));
        error_page.set_title(crate::tr_en!("Could not load observation"));
        let err_retry = pill_button(crate::tr_en!("Retry"));
        error_page.set_child(Some(&err_retry));
        stack.add_named(&error_page, Some("error"));

        // ── Success ────────────────────────────────────────────────────
        let success_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        success_container.set_vexpand(true);
        success_container.set_hexpand(true);
        stack.add_named(&success_container, Some("success"));

        stack.set_visible_child_name("loading");

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);
        widget.append(&stack);

        let page = Rc::new(ObservationDetailPage {
            widget,
            services,
            stack,
            success_container,
            notfound_page,
            error_page,
            current_publisher_id: RefCell::new(String::new()),
        });

        // Retry buttons re-run the current fetch (e.g. after signing in).
        {
            let p = Rc::clone(&page);
            auth_retry.connect_clicked(move |_| {
                let pid = p.current_publisher_id.borrow().clone();
                if !pid.is_empty() {
                    p.show(&pid);
                }
            });
        }
        {
            let p = Rc::clone(&page);
            err_retry.connect_clicked(move |_| {
                let pid = p.current_publisher_id.borrow().clone();
                if !pid.is_empty() {
                    p.show(&pid);
                }
            });
        }

        page
    }

    /// Root widget to embed in the host view.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Load (or reload) the detail view for a search-result publisher id.
    pub fn show(self: &Rc<Self>, publisher_id: &str) {
        *self.current_publisher_id.borrow_mut() = publisher_id.to_string();
        let this = Rc::clone(self);
        let pid = publisher_id.to_string();
        glib::spawn_future_local(async move {
            this.load(pid).await;
        });
    }

    async fn load(self: &Rc<Self>, publisher_id: String) {
        self.stack.set_visible_child_name("loading");

        let svc = self.services.clone();
        let endpoints = self.services.endpoints.clone();
        let pid = publisher_id.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let caom2 = CAOM2Service::new(reqwest::Client::new(), endpoints);
                caom2.get_by_publisher_id(token.as_deref(), &pid).await
            })
            .await;

        // Discard a stale fetch: the user may have opened a different observation
        // while this one was still in flight.
        if *self.current_publisher_id.borrow() != publisher_id {
            return;
        }

        match result.status {
            Caom2Status::Success => {
                if let Some(obs) = result.observation {
                    self.populate(&obs);
                    self.stack.set_visible_child_name("success");
                } else {
                    self.error_page.set_description(Some(crate::tr_en!(
                        "The service returned no observation."
                    )));
                    self.stack.set_visible_child_name("error");
                }
            }
            Caom2Status::AuthRequired => {
                self.stack.set_visible_child_name("auth");
            }
            Caom2Status::NotFound | Caom2Status::InvalidId => {
                self.notfound_page
                    .set_description(Some(&format!("No observation found for {}", publisher_id)));
                self.stack.set_visible_child_name("notfound");
            }
            Caom2Status::Parse | Caom2Status::ServerError => {
                let msg = result
                    .error
                    .unwrap_or_else(|| crate::tr_en!("The metadata service is unreachable.").to_string());
                self.error_page.set_description(Some(&msg));
                self.stack.set_visible_child_name("error");
            }
        }
    }

    /// Rebuild the Success view (header + 5-tab notebook) for `obs`.
    fn populate(&self, obs: &CAOM2Observation) {
        while let Some(child) = self.success_container.first_child() {
            self.success_container.remove(&child);
        }

        self.success_container.append(&build_header(obs));

        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.set_hexpand(true);
        notebook.set_scrollable(true);

        let pid = self.current_publisher_id.borrow().clone();

        notebook.append_page(
            &build_overview(obs),
            Some(&gtk::Label::new(Some(crate::tr_en!("Overview")))),
        );
        notebook.append_page(
            &build_coverage(obs),
            Some(&gtk::Label::new(Some(crate::tr_en!("Coverage")))),
        );
        notebook.append_page(
            &build_files(obs, &self.services, &self.widget, &pid),
            Some(&gtk::Label::new(Some(crate::tr_en!("Files")))),
        );
        notebook.append_page(
            &build_provenance(obs),
            Some(&gtk::Label::new(Some(crate::tr_en!("Provenance")))),
        );
        notebook.append_page(
            &build_raw(obs),
            Some(&gtk::Label::new(Some(crate::tr_en!("Raw")))),
        );

        self.success_container.append(&notebook);
    }
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn build_header(obs: &CAOM2Observation) -> gtk::Box {
    let hb = gtk::Box::new(gtk::Orientation::Vertical, 4);
    hb.set_margin_start(16);
    hb.set_margin_end(16);
    hb.set_margin_top(12);
    hb.set_margin_bottom(8);

    let title = gtk::Label::new(Some(&obs.observation_id));
    title.add_css_class("title-2");
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_selectable(true);
    hb.append(&title);

    if !obs.collection.is_empty() {
        let sub = gtk::Label::new(Some(&obs.collection));
        sub.add_css_class("dim-label");
        sub.add_css_class("caption");
        sub.set_halign(gtk::Align::Start);
        hb.append(&sub);
    }

    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chips.set_halign(gtk::Align::Start);
    chips.set_margin_top(4);
    if let Some(t) = obs.observation_type.as_deref().filter(|s| !s.trim().is_empty()) {
        chips.append(&chip(t, "badge-bookmarked"));
    }
    if let Some(i) = obs.intent.as_deref().filter(|s| !s.trim().is_empty()) {
        let science = i.eq_ignore_ascii_case("science");
        chips.append(&chip(i, if science { "badge-fits" } else { "badge-bookmarked" }));
    }
    if chips.first_child().is_some() {
        hb.append(&chips);
    }

    hb
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

fn build_overview(obs: &CAOM2Observation) -> gtk::ScrolledWindow {
    let content = section_box();

    add_card(
        &content,
        crate::tr_en!("Identity"),
        &[
            (crate::tr_en!("Algorithm").into(), f_text(obs.algorithm.as_deref())),
            (crate::tr_en!("Sequence Number").into(), f_text(obs.sequence_number.as_deref())),
            (crate::tr_en!("Type").into(), f_text(obs.observation_type.as_deref())),
            (crate::tr_en!("Intent").into(), f_text(obs.intent.as_deref())),
        ],
    );

    if let Some(t) = &obs.target {
        add_card(
            &content,
            crate::tr_en!("Target"),
            &[
                (crate::tr_en!("Name").into(), f_text(t.name.as_deref())),
                (crate::tr_en!("Type").into(), f_text(t.kind.as_deref())),
                (crate::tr_en!("Standard").into(), f_bool(t.standard)),
                (crate::tr_en!("Redshift").into(), f_number(t.redshift)),
                (crate::tr_en!("Moving").into(), f_bool(t.moving)),
                (crate::tr_en!("Keywords").into(), join_keywords(&t.keywords)),
            ],
        );
    }

    if let Some(p) = &obs.proposal {
        add_card(
            &content,
            crate::tr_en!("Proposal"),
            &[
                (crate::tr_en!("ID").into(), f_text(p.id.as_deref())),
                (crate::tr_en!("PI").into(), f_text(p.pi.as_deref())),
                (crate::tr_en!("Project").into(), f_text(p.project.as_deref())),
                (crate::tr_en!("Title").into(), f_text(p.title.as_deref())),
                (crate::tr_en!("Keywords").into(), join_keywords(&p.keywords)),
            ],
        );
    }

    if obs.telescope.is_some() || obs.instrument.is_some() {
        let geo = obs
            .telescope
            .as_ref()
            .and_then(|t| t.geo_location)
            .map(|(x, y, z)| format!("({}, {}, {}) m", trim_float(x, 4), trim_float(y, 4), trim_float(z, 4)))
            .unwrap_or_else(|| DASH.to_string());
        add_card(
            &content,
            crate::tr_en!("Telescope & Instrument"),
            &[
                (
                    crate::tr_en!("Telescope").into(),
                    f_text(obs.telescope.as_ref().and_then(|t| t.name.as_deref())),
                ),
                (
                    crate::tr_en!("Instrument").into(),
                    f_text(obs.instrument.as_ref().and_then(|i| i.name.as_deref())),
                ),
                (crate::tr_en!("Location").into(), geo),
            ],
        );
    }

    if let Some(e) = &obs.environment {
        add_card(
            &content,
            crate::tr_en!("Environment"),
            &[
                (crate::tr_en!("Seeing").into(), f_number(e.seeing)),
                (crate::tr_en!("Humidity").into(), f_number(e.humidity)),
                (crate::tr_en!("Elevation").into(), f_degrees(e.elevation)),
                (crate::tr_en!("Tau").into(), f_number(e.tau)),
            ],
        );
    }

    if content.first_child().is_none() {
        content.append(&dim_label(crate::tr_en!("No overview information available.")));
    }

    scrolled(&content)
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

fn build_coverage(obs: &CAOM2Observation) -> gtk::ScrolledWindow {
    let content = section_box();

    if obs.planes.is_empty() {
        content.append(&dim_label(crate::tr_en!("No coverage information available.")));
        return scrolled(&content);
    }

    let multi = obs.planes.len() > 1;
    for (i, plane) in obs.planes.iter().enumerate() {
        let plane_box = gtk::Box::new(gtk::Orientation::Vertical, 12);

        // Spatial footprint (drawn polygon + RA/Dec bounding box).
        if !plane.position_bounds.is_empty() {
            if let Some(fp) = build_footprint(&plane.position_bounds) {
                plane_box.append(&fp);
            }
            let (min_ra, max_ra, min_dec, max_dec) = bbox(&plane.position_bounds);
            add_card(
                &plane_box,
                crate::tr_en!("Spatial Footprint"),
                &[
                    (
                        crate::tr_en!("RA range").into(),
                        format!("{} – {}", f_degrees(Some(min_ra)), f_degrees(Some(max_ra))),
                    ),
                    (
                        crate::tr_en!("Dec range").into(),
                        format!("{} – {}", f_degrees(Some(min_dec)), f_degrees(Some(max_dec))),
                    ),
                    (crate::tr_en!("Vertices").into(), plane.position_bounds.len().to_string()),
                ],
            );
        }

        // Spectral.
        if plane.energy_lower.is_some() || plane.energy_upper.is_some() {
            add_card(
                &plane_box,
                crate::tr_en!("Spectral"),
                &[(
                    crate::tr_en!("Wavelength").into(),
                    f_wavelength_range(plane.energy_lower, plane.energy_upper),
                )],
            );
        }

        // Temporal (MJD → calendar UTC).
        if plane.time_lower.is_some() || plane.time_upper.is_some() {
            add_card(
                &plane_box,
                crate::tr_en!("Temporal"),
                &[
                    (crate::tr_en!("Start").into(), f_mjd_to_date(plane.time_lower)),
                    (crate::tr_en!("End").into(), f_mjd_to_date(plane.time_upper)),
                ],
            );
        }

        // Plane identity.
        add_card(
            &plane_box,
            crate::tr_en!("Plane"),
            &[
                (crate::tr_en!("Product ID").into(), f_text(Some(&plane.product_id))),
                (crate::tr_en!("Data Product Type").into(), f_text(plane.data_product_type.as_deref())),
                (
                    crate::tr_en!("Calibration Level").into(),
                    plane.calibration_level.map(|c| c.to_string()).unwrap_or_else(|| DASH.to_string()),
                ),
                (crate::tr_en!("Quality").into(), f_text(plane.quality.as_deref())),
            ],
        );

        append_plane(&content, plane_box, plane_expander_title(plane), multi, i == 0);
    }

    scrolled(&content)
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

fn build_files(
    obs: &CAOM2Observation,
    services: &Arc<AppServices>,
    root: &gtk::Box,
    publisher_id: &str,
) -> gtk::ScrolledWindow {
    let content = section_box();

    if obs.planes.is_empty() {
        content.append(&dim_label(crate::tr_en!("No files available.")));
    } else {
        let multi = obs.planes.len() > 1;
        for (i, plane) in obs.planes.iter().enumerate() {
            let plane_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
            if plane.artifacts.is_empty() {
                plane_box.append(&dim_label(crate::tr_en!("No files in this plane.")));
            } else {
                for art in &plane.artifacts {
                    plane_box.append(&build_artifact_row(art, services, root, publisher_id));
                }
            }
            let title = format!("{} · {} file(s)", plane.product_id, plane.artifacts.len());
            append_plane(&content, plane_box, title, multi, i == 0);
        }
    }

    // "View all files on CADC" opens the archive search in the default browser.
    let link = gtk::Button::with_label(crate::tr_en!("View all files on CADC"));
    link.add_css_class("flat");
    link.set_halign(gtk::Align::Start);
    link.set_margin_top(6);
    link.connect_clicked(|_| {
        let _ = open::that("https://www.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/en/search/");
    });
    content.append(&link);

    scrolled(&content)
}

fn build_artifact_row(
    art: &Caom2Artifact,
    services: &Arc<AppServices>,
    root: &gtk::Box,
    publisher_id: &str,
) -> gtk::Frame {
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    inner.set_margin_start(10);
    inner.set_margin_end(10);
    inner.set_margin_top(8);
    inner.set_margin_bottom(8);

    let ptype = art.product_type.clone().unwrap_or_default();
    let badge_text = if ptype.trim().is_empty() {
        crate::tr_en!("file").to_string()
    } else {
        ptype.clone()
    };
    let badge = chip(&badge_text, artifact_badge_class(&ptype));
    inner.append(&badge);

    let fname = artifact_file_name(&art.uri);
    let name = gtk::Label::new(Some(&fname));
    name.set_hexpand(true);
    name.set_halign(gtk::Align::Start);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    name.set_selectable(true);
    name.set_tooltip_text(Some(&art.uri));
    inner.append(&name);

    let mut meta_text = String::new();
    if let Some(ct) = art.content_type.as_deref().filter(|s| !s.trim().is_empty()) {
        meta_text.push_str(ct);
        meta_text.push_str("  ");
    }
    meta_text.push_str(&f_bytes(art.content_length));
    let meta = gtk::Label::new(Some(&meta_text));
    meta.add_css_class("dim-label");
    meta.add_css_class("caption");
    meta.set_valign(gtk::Align::Center);
    inner.append(&meta);

    let dl = gtk::Button::from_icon_name("folder-download-symbolic");
    dl.add_css_class("flat");
    dl.set_valign(gtk::Align::Center);
    dl.set_tooltip_text(Some(crate::tr_en!("Download this file")));
    {
        let services = services.clone();
        let root = root.clone();
        let pid = publisher_id.to_string();
        let uri = art.uri.clone();
        let pt = art.product_type.clone();
        dl.connect_clicked(move |_| {
            let services = services.clone();
            let root = root.clone();
            let pid = pid.clone();
            let uri = uri.clone();
            let pt = pt.clone();
            glib::spawn_future_local(async move {
                download_artifact(services, root, pid, uri, pt).await;
            });
        });
    }
    inner.append(&dl);

    let frame = gtk::Frame::new(None);
    frame.add_css_class("card");
    frame.set_child(Some(&inner));
    frame
}

/// Best-effort artifact download: resolve DataLink, pick the matching URL, stream
/// to the user's Downloads directory, then open FITS files in the viewer (via the
/// `app.open-fits-file` action) or hand other files to the OS default app.
async fn download_artifact(
    services: Arc<AppServices>,
    root: gtk::Box,
    publisher_id: String,
    artifact_uri: String,
    product_type: Option<String>,
) {
    let filename = artifact_file_name(&artifact_uri);
    services.toast.toast(&format!("Downloading {}…", filename));

    // 1. Resolve DataLink for this observation.
    let svc = services.clone();
    let pid = publisher_id.clone();
    let dl = match services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink.resolve(&pid, token.as_deref()).await
        })
        .await
    {
        Ok(d) => d,
        Err(e) => {
            services.toast.toast(&format!("Download failed: {}", e));
            return;
        }
    };

    let is_preview = matches!(
        product_type.as_deref().map(str::to_lowercase).as_deref(),
        Some("preview") | Some("thumbnail")
    );
    let url = match pick_url(&dl, &filename, is_preview) {
        Some(u) => u,
        None => {
            services.toast.toast(&format!("No download link for {}", filename));
            return;
        }
    };

    // Choose a target filename/path.
    let mut name = filename.clone();
    if !name.contains('.') {
        name.push_str(".fits");
    }
    let target = download_dir().join(&name);

    // 2. Download and write on the tokio pool (keeps the main loop responsive).
    let svc = services.clone();
    let url2 = url.clone();
    let target2 = target.clone();
    let write_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            match svc.datalink.download_file(&url2, token.as_deref()).await {
                Ok((bytes, _)) => match tokio::task::spawn_blocking(move || std::fs::write(&target2, &bytes)).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
                Err(e) => Err(e.to_string()),
            }
        })
        .await;

    match write_result {
        Ok(()) => {
            let path_str = target.to_string_lossy().to_string();
            if is_fits_name(&name) {
                services
                    .toast
                    .toast(&format!("Downloaded {} — opening in FITS viewer", name));
                activate_open_fits(&root, &path_str);
            } else {
                services.toast.toast(&format!("Downloaded to {}", path_str));
                let _ = open::that(&target);
            }
        }
        Err(e) => services.toast.toast(&format!("Download failed: {}", e)),
    }
}

fn pick_url(dl: &DataLinkResult, filename: &str, is_preview: bool) -> Option<String> {
    let fnl = filename.to_lowercase();
    if let Some(f) = dl
        .files
        .iter()
        .find(|f| f.filename().to_lowercase() == fnl || f.url.to_lowercase().contains(&fnl))
    {
        return Some(f.url.clone());
    }
    if is_preview {
        if let Some(f) = dl.files.iter().find(|f| f.is_thumbnail() || f.is_preview()) {
            return Some(f.url.clone());
        }
    } else if let Some(f) = dl.files.iter().find(|f| f.is_science_data()) {
        return Some(f.url.clone());
    }
    dl.files
        .first()
        .map(|f| f.url.clone())
        .or_else(|| dl.download_url.clone())
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

fn build_provenance(obs: &CAOM2Observation) -> gtk::ScrolledWindow {
    let content = section_box();

    add_card(
        &content,
        crate::tr_en!("Processing"),
        &[
            (crate::tr_en!("Algorithm").into(), f_text(obs.algorithm.as_deref())),
            (crate::tr_en!("Sequence Number").into(), f_text(obs.sequence_number.as_deref())),
            (crate::tr_en!("Observation Type").into(), f_text(obs.observation_type.as_deref())),
            (crate::tr_en!("Intent").into(), f_text(obs.intent.as_deref())),
        ],
    );

    if !obs.planes.is_empty() {
        let rows: Vec<(String, String)> = obs
            .planes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    format!("Plane {}", i + 1),
                    format!(
                        "{} (L{}, {})",
                        p.product_id,
                        p.calibration_level.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string()),
                        f_text(p.data_product_type.as_deref())
                    ),
                )
            })
            .collect();
        add_card(&content, crate::tr_en!("Planes"), &rows);
    }

    if content.first_child().is_none() {
        content.append(&dim_label(crate::tr_en!("No provenance information available.")));
    }

    scrolled(&content)
}

// ---------------------------------------------------------------------------
// Raw
// ---------------------------------------------------------------------------

fn build_raw(obs: &CAOM2Observation) -> gtk::ScrolledWindow {
    let mut lines: Vec<String> = Vec::new();

    push_raw(&mut lines, "collection", f_text(Some(&obs.collection)));
    push_raw(&mut lines, "observationID", f_text(Some(&obs.observation_id)));
    push_raw(&mut lines, "type", f_text(obs.observation_type.as_deref()));
    push_raw(&mut lines, "intent", f_text(obs.intent.as_deref()));
    push_raw(&mut lines, "sequenceNumber", f_text(obs.sequence_number.as_deref()));
    push_raw(&mut lines, "algorithm", f_text(obs.algorithm.as_deref()));
    push_raw(&mut lines, "target.name", f_text(obs.target.as_ref().and_then(|t| t.name.as_deref())));
    push_raw(&mut lines, "target.type", f_text(obs.target.as_ref().and_then(|t| t.kind.as_deref())));
    push_raw(&mut lines, "target.redshift", f_number(obs.target.as_ref().and_then(|t| t.redshift)));
    push_raw(&mut lines, "proposal.id", f_text(obs.proposal.as_ref().and_then(|p| p.id.as_deref())));
    push_raw(&mut lines, "proposal.pi", f_text(obs.proposal.as_ref().and_then(|p| p.pi.as_deref())));
    push_raw(&mut lines, "proposal.project", f_text(obs.proposal.as_ref().and_then(|p| p.project.as_deref())));
    push_raw(&mut lines, "proposal.title", f_text(obs.proposal.as_ref().and_then(|p| p.title.as_deref())));
    push_raw(&mut lines, "telescope.name", f_text(obs.telescope.as_ref().and_then(|t| t.name.as_deref())));
    push_raw(&mut lines, "instrument.name", f_text(obs.instrument.as_ref().and_then(|i| i.name.as_deref())));
    if let Some(e) = &obs.environment {
        push_raw(&mut lines, "environment.seeing", f_number(e.seeing));
        push_raw(&mut lines, "environment.humidity", f_number(e.humidity));
        push_raw(&mut lines, "environment.elevation", f_degrees(e.elevation));
        push_raw(&mut lines, "environment.tau", f_number(e.tau));
    }

    for (i, p) in obs.planes.iter().enumerate() {
        let prefix = format!("plane[{}].", i);
        push_raw(&mut lines, &format!("{}productID", prefix), f_text(Some(&p.product_id)));
        push_raw(&mut lines, &format!("{}dataProductType", prefix), f_text(p.data_product_type.as_deref()));
        push_raw(
            &mut lines,
            &format!("{}calibrationLevel", prefix),
            p.calibration_level.map(|c| c.to_string()).unwrap_or_else(|| DASH.to_string()),
        );
        push_raw(&mut lines, &format!("{}quality", prefix), f_text(p.quality.as_deref()));
        push_raw(&mut lines, &format!("{}energy.lower", prefix), f_wavelength(p.energy_lower));
        push_raw(&mut lines, &format!("{}energy.upper", prefix), f_wavelength(p.energy_upper));
        push_raw(&mut lines, &format!("{}time.lower", prefix), f_mjd_to_date(p.time_lower));
        push_raw(&mut lines, &format!("{}time.upper", prefix), f_mjd_to_date(p.time_upper));
        push_raw(&mut lines, &format!("{}artifacts", prefix), p.artifacts.len().to_string());
    }

    let content = section_box();
    let text = if lines.is_empty() {
        crate::tr_en!("No data.").to_string()
    } else {
        lines.join("\n")
    };
    let label = gtk::Label::new(Some(&text));
    label.add_css_class("monospace");
    label.set_selectable(true);
    label.set_wrap(false);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
    content.append(&label);

    scrolled(&content)
}

fn push_raw(lines: &mut Vec<String>, key: &str, value: String) {
    if value != DASH {
        lines.push(format!("{:<26} {}", format!("{}:", key), value));
    }
}

// ---------------------------------------------------------------------------
// Footprint drawing
// ---------------------------------------------------------------------------

/// Draw the sky footprint polygon (RA mirrored so it increases to the left, Dec
/// increasing upward), scaled to the drawing area with a 10% padding. Mirrors
/// `BuildFootprint` in the reference.
fn build_footprint(bounds: &[(f64, f64)]) -> Option<gtk::Frame> {
    if bounds.len() < 3 {
        return None;
    }
    let area = gtk::DrawingArea::new();
    area.set_content_width(220);
    area.set_content_height(140);

    let pts = bounds.to_vec();
    area.set_draw_func(move |_a, cr, w, h| {
        let (min_ra, max_ra, min_dec, max_dec) = bbox(&pts);
        let range_ra = (max_ra - min_ra).max(1e-9);
        let range_dec = (max_dec - min_dec).max(1e-9);
        let pad = 0.1_f64;
        let wf = w as f64;
        let hf = h as f64;

        for (i, p) in pts.iter().enumerate() {
            let nx = (max_ra - p.0) / range_ra; // mirror RA
            let ny = (max_dec - p.1) / range_dec; // Dec increases upward
            let x = pad * wf + nx * (1.0 - 2.0 * pad) * wf;
            let y = pad * hf + ny * (1.0 - 2.0 * pad) * hf;
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.close_path();

        cr.set_source_rgba(0.35, 0.55, 0.95, 0.18);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.20, 0.50, 0.95, 0.9);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    });

    let frame = gtk::Frame::new(None);
    frame.add_css_class("card");
    frame.set_halign(gtk::Align::Start);
    frame.set_child(Some(&area));
    Some(frame)
}

fn bbox(pts: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let min_ra = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let max_ra = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let min_dec = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_dec = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    (min_ra, max_ra, min_dec, max_dec)
}

// ---------------------------------------------------------------------------
// Small widget helpers
// ---------------------------------------------------------------------------

fn section_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.set_margin_start(16);
    b.set_margin_end(16);
    b.set_margin_top(16);
    b.set_margin_bottom(16);
    b
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_vexpand(true);
    s.set_hexpand(true);
    s.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    s.set_child(Some(child));
    s
}

fn dim_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    l.set_halign(gtk::Align::Start);
    l.set_wrap(true);
    l
}

fn chip(text: &str, css: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(css);
    l.add_css_class("caption");
    l.set_valign(gtk::Align::Center);
    l
}

fn pill_button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    b.add_css_class("suggested-action");
    b.set_halign(gtk::Align::Center);
    b
}

/// Append a plane's content to a tab: bare when single, wrapped in an `Expander`
/// (first one expanded) when multiple planes exist.
fn append_plane(container: &gtk::Box, plane_box: gtk::Box, title: String, multi: bool, expanded: bool) {
    if multi {
        let exp = gtk::Expander::new(Some(&title));
        exp.set_hexpand(true);
        exp.set_expanded(expanded);
        exp.set_child(Some(&plane_box));
        container.append(&exp);
    } else {
        container.append(&plane_box);
    }
}

fn plane_expander_title(plane: &crate::models::caom2::Caom2Plane) -> String {
    format!(
        "{} · {} · L{}",
        plane.product_id,
        f_text(plane.data_product_type.as_deref()),
        plane.calibration_level.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string())
    )
}

/// Build a titled card (an `adw::PreferencesGroup` of label/value rows), skipping
/// dashed/empty values and the whole card when nothing survives. Appends to
/// `container`.
fn add_card(container: &gtk::Box, title: &str, rows: &[(String, String)]) {
    let visible: Vec<&(String, String)> = rows
        .iter()
        .filter(|(_, v)| v.as_str() != DASH && !v.trim().is_empty())
        .collect();
    if visible.is_empty() {
        return;
    }
    let group = adw::PreferencesGroup::new();
    group.set_title(title);
    for (label, value) in visible {
        let row = adw::ActionRow::builder()
            .title(label.as_str())
            .subtitle(value.as_str())
            .subtitle_selectable(true)
            .build();
        group.add(&row);
    }
    container.append(&group);
}

fn artifact_badge_class(product_type: &str) -> &'static str {
    match product_type.to_lowercase().as_str() {
        "science" => "badge-fits",
        _ => "badge-bookmarked",
    }
}

/// Activate the app-level `open-fits-file` action with a downloaded path.
fn activate_open_fits(widget: &gtk::Box, path: &str) {
    if let Some(root) = widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        if let Some(app) = root.application() {
            let ag: &gtk::gio::ActionGroup = app.upcast_ref();
            ag.activate_action("open-fits-file", Some(&glib::Variant::from(path)));
        }
    }
}

fn download_dir() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// Last path segment of a `cadc:`/`vos:` artifact URI (the file name).
fn artifact_file_name(uri: &str) -> String {
    if uri.is_empty() {
        return uri.to_string();
    }
    match uri.rfind('/') {
        Some(i) if i + 1 < uri.len() => uri[i + 1..].to_string(),
        _ => uri.to_string(),
    }
}

fn is_fits_name(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".fits") || l.ends_with(".fit") || l.ends_with(".fts") || l.ends_with(".fz")
}

// ---------------------------------------------------------------------------
// Value formatting (mirrors Helpers/Caom2Format.cs)
// ---------------------------------------------------------------------------

fn f_text(s: Option<&str>) -> String {
    match s {
        Some(v) if !v.trim().is_empty() => v.to_string(),
        _ => DASH.to_string(),
    }
}

fn f_bool(b: Option<bool>) -> String {
    match b {
        Some(true) => crate::tr_en!("Yes").to_string(),
        Some(false) => crate::tr_en!("No").to_string(),
        None => DASH.to_string(),
    }
}

fn f_number(d: Option<f64>) -> String {
    match d {
        Some(v) if v.is_finite() => trim_float(v, 4),
        _ => DASH.to_string(),
    }
}

fn f_degrees(d: Option<f64>) -> String {
    match d {
        Some(v) if v.is_finite() => format!("{}°", trim_float(v, 6)),
        _ => DASH.to_string(),
    }
}

/// Human-readable byte size (B/KB/MB/GB/TB).
fn f_bytes(bytes: Option<u64>) -> String {
    let b = match bytes {
        Some(b) => b,
        None => return DASH.to_string(),
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = b as f64;
    let mut u = 0;
    while size >= 1024.0 && u < units.len() - 1 {
        size /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", b, units[u])
    } else {
        format!("{} {}", trim_float(size, 1), units[u])
    }
}

/// Wavelength in metres → friendly nm/µm/mm/m.
fn f_wavelength(metres: Option<f64>) -> String {
    let m = match metres {
        Some(m) if m > 0.0 && m.is_finite() => m,
        _ => return DASH.to_string(),
    };
    if m < 1e-6 {
        format!("{} nm", trim_float(m * 1e9, 3))
    } else if m < 1e-3 {
        format!("{} µm", trim_float(m * 1e6, 3))
    } else if m < 1.0 {
        format!("{} mm", trim_float(m * 1e3, 3))
    } else {
        format!("{} m", trim_float(m, 3))
    }
}

fn f_wavelength_range(lower: Option<f64>, upper: Option<f64>) -> String {
    if lower.is_none() && upper.is_none() {
        DASH.to_string()
    } else {
        format!("{} – {}", f_wavelength(lower), f_wavelength(upper))
    }
}

/// MJD (epoch 1858-11-17 UTC) → calendar UTC string.
fn f_mjd_to_date(mjd: Option<f64>) -> String {
    let v = match mjd {
        Some(v) if v.is_finite() => v,
        _ => return DASH.to_string(),
    };
    let epoch = chrono::NaiveDate::from_ymd_opt(1858, 11, 17)
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match epoch {
        Some(e) => {
            let dt = e + chrono::Duration::milliseconds((v * 86_400_000.0) as i64);
            dt.format("%Y-%m-%d %H:%M UTC").to_string()
        }
        None => DASH.to_string(),
    }
}

fn join_keywords(keywords: &[String]) -> String {
    if keywords.is_empty() {
        DASH.to_string()
    } else {
        keywords.join(", ")
    }
}

/// Format a float to at most `decimals` places, trimming trailing zeros and a
/// dangling decimal point (approximates .NET's "0.###" formatting).
fn trim_float(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    } else {
        s
    }
}
