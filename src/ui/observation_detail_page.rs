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
            (crate::tr_en!("Meta Release").into(), f_date(obs.meta_release.as_deref())),
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
                (crate::tr_en!("Ambient Temp").into(), f_number(e.ambient_temp)),
                (crate::tr_en!("Photometric").into(), f_bool(e.photometric)),
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

        // Spatial: drawn footprint + RA/Dec bounding box + pixel / resolution detail.
        {
            if let Some(fp) = build_footprint(&plane.position_bounds) {
                plane_box.append(&fp);
            }
            let mut rows: Vec<(String, String)> = Vec::new();
            if !plane.position_bounds.is_empty() {
                let (min_ra, max_ra, min_dec, max_dec) = bbox(&plane.position_bounds);
                rows.push((
                    crate::tr_en!("RA range").into(),
                    format!("{} – {}", f_degrees(Some(min_ra)), f_degrees(Some(max_ra))),
                ));
                rows.push((
                    crate::tr_en!("Dec range").into(),
                    format!("{} – {}", f_degrees(Some(min_dec)), f_degrees(Some(max_dec))),
                ));
                rows.push((crate::tr_en!("Vertices").into(), plane.position_bounds.len().to_string()));
            }
            if let Some((a, b)) = plane.position_dimension {
                rows.push((crate::tr_en!("Dimensions").into(), format!("{} × {} px", a, b)));
            }
            if let Some(r) = plane.position_resolution {
                rows.push((crate::tr_en!("Resolution").into(), format!("{}″", trim_float(r, 4))));
            }
            if let Some(s) = plane.position_sample_size {
                rows.push((crate::tr_en!("Sample Size").into(), format!("{}″", trim_float(s, 4))));
            }
            add_card(&plane_box, crate::tr_en!("Spatial Footprint"), &rows);
        }

        // Spectral (bandpass / band / wavelength range / resolving power / rest wavelength).
        add_card(
            &plane_box,
            crate::tr_en!("Spectral"),
            &[
                (crate::tr_en!("Bandpass").into(), f_text(plane.energy_bandpass.as_deref())),
                (crate::tr_en!("Band").into(), f_text(plane.energy_em_band.as_deref())),
                (
                    crate::tr_en!("Wavelength").into(),
                    f_wavelength_range(plane.energy_lower, plane.energy_upper),
                ),
                (crate::tr_en!("Resolving Power").into(), f_number(plane.energy_resolving_power)),
                (crate::tr_en!("Rest Wavelength").into(), f_wavelength(plane.energy_rest_wav)),
            ],
        );

        // Temporal (MJD → calendar UTC; exposure via caom2_format::seconds).
        add_card(
            &plane_box,
            crate::tr_en!("Temporal"),
            &[
                (crate::tr_en!("Start").into(), f_mjd_to_date(plane.time_lower)),
                (crate::tr_en!("End").into(), f_mjd_to_date(plane.time_upper)),
                (
                    crate::tr_en!("Exposure").into(),
                    crate::helpers::caom2_format::seconds(plane.time_exposure),
                ),
            ],
        );

        // Polarization states.
        if !plane.polarization_states.is_empty() {
            add_card(
                &plane_box,
                crate::tr_en!("Polarization"),
                &[(
                    crate::tr_en!("States").into(),
                    plane.polarization_states.join(", "),
                )],
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

    // Snapshot the observation metadata a download needs to register itself in the
    // Research library. Captured up front (owned) so the async download closure is
    // independent of the page's lifetime — the page is reused across observations,
    // so reading live fields at completion could stamp the wrong record.
    let meta = ObsMeta {
        publisher_id: publisher_id.to_string(),
        collection: obs.collection.clone(),
        observation_id: obs.observation_id.clone(),
        target_name: obs
            .target
            .as_ref()
            .and_then(|t| t.name.clone())
            .unwrap_or_default(),
        instrument: obs
            .instrument
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_default(),
    };

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
                    plane_box.append(&build_artifact_row(art, services, root, &meta));
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
    meta: &ObsMeta,
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
    let meta_label = gtk::Label::new(Some(&meta_text));
    meta_label.add_css_class("dim-label");
    meta_label.add_css_class("caption");
    meta_label.set_valign(gtk::Align::Center);
    inner.append(&meta_label);

    // Inline status area (progress / viewer buttons / "Added to Research" banner).
    // Hidden until a download starts; each artifact row owns its own so parallel
    // downloads never overwrite each other's status.
    let status_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    status_box.set_margin_start(10);
    status_box.set_margin_end(10);
    status_box.set_margin_bottom(8);
    status_box.set_visible(false);

    let dl = gtk::Button::from_icon_name("folder-download-symbolic");
    dl.add_css_class("flat");
    dl.set_valign(gtk::Align::Center);
    dl.set_tooltip_text(Some(crate::tr_en!("Download this file")));
    {
        let services = services.clone();
        let root = root.clone();
        let meta = meta.clone();
        let uri = art.uri.clone();
        let pt = art.product_type.clone();
        let status_box = status_box.clone();
        dl.connect_clicked(move |btn| {
            // Guard against double-clicks: the streamed download can take a while.
            btn.set_sensitive(false);
            let btn = btn.clone();
            let services = services.clone();
            let root = root.clone();
            let meta = meta.clone();
            let uri = uri.clone();
            let pt = pt.clone();
            let status_box = status_box.clone();
            glib::spawn_future_local(async move {
                download_artifact(services, root, status_box, meta, uri, pt).await;
                btn.set_sensitive(true);
            });
        });
    }
    inner.append(&dl);

    // Frame wraps a vertical box: the file row on top, its status area below.
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(&inner);
    outer.append(&status_box);

    let frame = gtk::Frame::new(None);
    frame.add_css_class("card");
    frame.set_child(Some(&outer));
    frame
}

/// Owned snapshot of the observation metadata a download needs to register a
/// Research-library record, captured when the Files tab is built. The page is
/// reused across observations, so the download closure must not read live fields.
#[derive(Clone)]
struct ObsMeta {
    publisher_id: String,
    collection: String,
    observation_id: String,
    target_name: String,
    instrument: String,
}

/// Download an artifact into the managed Research library, register the science
/// file so it appears in Research, and recommend a viewer by inspecting the FITS
/// content (spectral cube → 3D Cube Viewer, else 2D FITS Viewer).
///
/// Mirrors the Windows `OnDownloadArtifactAsync` / `DownloadUrlToFileAsync` /
/// `RegisterInResearch` / `BuildViewerButton` flow. Reuses the same streaming
/// helper the Search / Research pages use (`stream_download_to_file`, chunked to
/// a sibling `.tmp` then renamed) and the same `open-fits-file` / `open-cube-file`
/// / `navigate-research` app actions the Research page fires.
async fn download_artifact(
    services: Arc<AppServices>,
    root: gtk::Box,
    status_box: gtk::Box,
    meta: ObsMeta,
    artifact_uri: String,
    product_type: Option<String>,
) {
    let filename = artifact_file_name(&artifact_uri);
    status_downloading(&status_box, &format!("Resolving {}…", filename));

    // 1. Resolve DataLink for this observation.
    let svc = services.clone();
    let pid = meta.publisher_id.clone();
    let dl = match services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink.resolve(&pid, token.as_deref()).await
        })
        .await
    {
        Ok(d) => d,
        Err(e) => {
            status_error(&status_box, &format!("Download failed: {}", e));
            return;
        }
    };

    // Branch on product type FIRST so a preview click never grabs the science URL
    // (and vice versa) — mirrors the reference URL-selection guard.
    let pt_lower = product_type.as_deref().map(|s| s.trim().to_lowercase());
    let is_preview = matches!(pt_lower.as_deref(), Some("preview") | Some("thumbnail"));
    let url = match pick_url(&dl, &filename, is_preview) {
        Some(u) => u,
        None => {
            status_error(&status_box, &format!("No download link for {}", filename));
            return;
        }
    };

    // 2. Destination under the managed Research directory (NOT ~/Downloads). The id
    //    is deterministic per publisher DID, so the download lands in — and later
    //    registers under — the same slot a Search-page save would use.
    let obs_id = uuid_from_publisher_id(&meta.publisher_id);
    let managed_dir = crate::services::managed_dir_for(&obs_id);
    if let Err(e) = std::fs::create_dir_all(&managed_dir) {
        status_error(&status_box, &format!("Cannot create storage directory: {}", e));
        return;
    }
    let mut name = filename.clone();
    if !name.contains('.') {
        name.push_str(".fits");
    }
    let target = managed_dir.join(&name);

    // 3. Stream the body chunk-by-chunk to disk (same helper the Search/Research
    //    pages use). Progress surfaces as throttled toasts; the inline spinner
    //    stays up for the duration.
    status_downloading(&status_box, &format!("Downloading {}…", name));
    let svc = services.clone();
    let url2 = url.clone();
    let dest = target.clone();
    let toast_handle = services.toast.clone();
    let progress_label = name.clone();
    let dl_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            crate::ui::search_page::stream_download_to_file(
                &url2,
                token.as_deref(),
                &dest,
                &toast_handle,
                &progress_label,
            )
            .await
        })
        .await;

    let file_size = match dl_result {
        Ok(n) => n,
        Err(e) => {
            status_error(&status_box, &format!("Download failed: {}", e));
            return;
        }
    };

    let path_str = target.to_string_lossy().to_string();

    // 4. Classify by CONTENT, not extension — a mis-served download can put FITS
    //    bytes in a ".png" (and vice versa). Runs off the GLib thread.
    let dest = target.clone();
    let shape = services
        .spawn(async move {
            tokio::task::spawn_blocking(move || crate::helpers::fits_sniff::inspect(&dest))
                .await
                .ok()
        })
        .await;

    match shape {
        Some(shape) if shape.kind != crate::helpers::fits_sniff::FitsKind::NotFits => {
            // Only the SCIENCE file is registered — a calibration/aux FITS would
            // clobber the science record (the store replaces by id/publisher DID).
            let is_science = !is_preview
                && matches!(pt_lower.as_deref(), None | Some("") | Some("science"));
            let registered = if is_science {
                register_in_research(&services, &meta, &dl, &path_str, file_size).await
            } else {
                false
            };
            status_downloaded_fits(&status_box, &root, &name, &path_str, shape, registered);
        }
        _ => {
            // Preview PNG / README / other sidecar — offer an OS-default open.
            status_downloaded_other(&status_box, &name, &path_str);
        }
    }
}

/// Register a downloaded science file in the Research library so it appears there.
/// Uses a deterministic id derived from the publisher DID, so re-downloading (or a
/// prior Search-page save) updates the same record rather than duplicating it, and
/// preserves any previously-cached preview. Best-effort; returns whether it saved.
async fn register_in_research(
    services: &Arc<AppServices>,
    meta: &ObsMeta,
    dl: &DataLinkResult,
    local_path: &str,
    file_size: u64,
) -> bool {
    let obs_id = uuid_from_publisher_id(&meta.publisher_id);

    // Preview URLs from DataLink so Research can show the image (falls back to any
    // existing record's cached values — the store swaps by id).
    let thumbnail_url = dl
        .files
        .iter()
        .find(|f| f.is_thumbnail())
        .map(|f| f.url.clone())
        .unwrap_or_default();
    let preview_url = dl
        .files
        .iter()
        .find(|f| f.is_preview())
        .map(|f| f.url.clone())
        .unwrap_or_default();

    // Preserve a prior save's cached preview path / URLs when present.
    let svc = services.clone();
    let pid = meta.publisher_id.clone();
    let existing = services
        .spawn(async move {
            svc.observation_store
                .load_async()
                .await
                .into_iter()
                .find(|o| o.publisher_id == pid)
        })
        .await;

    let obs = crate::services::DownloadedObservation {
        id: obs_id,
        publisher_id: meta.publisher_id.clone(),
        collection: meta.collection.clone(),
        observation_id: meta.observation_id.clone(),
        target_name: meta.target_name.clone(),
        instrument: meta.instrument.clone(),
        filter: existing.as_ref().map(|o| o.filter.clone()).unwrap_or_default(),
        ra: existing.as_ref().map(|o| o.ra.clone()).unwrap_or_default(),
        dec: existing.as_ref().map(|o| o.dec.clone()).unwrap_or_default(),
        start_date: existing.as_ref().map(|o| o.start_date.clone()).unwrap_or_default(),
        cal_level: existing.as_ref().map(|o| o.cal_level.clone()).unwrap_or_default(),
        local_path: local_path.to_string(),
        file_size,
        downloaded_at: chrono::Utc::now().to_rfc3339(),
        thumbnail_url: if thumbnail_url.is_empty() {
            existing.as_ref().map(|o| o.thumbnail_url.clone()).unwrap_or_default()
        } else {
            thumbnail_url
        },
        preview_url: if preview_url.is_empty() {
            existing.as_ref().map(|o| o.preview_url.clone()).unwrap_or_default()
        } else {
            preview_url
        },
        local_preview_path: existing
            .as_ref()
            .map(|o| o.local_preview_path.clone())
            .unwrap_or_default(),
        agent_attribution: None,
    };

    let svc = services.clone();
    matches!(
        services
            .spawn(async move { svc.observation_store.save_async(obs).await })
            .await,
        Ok(())
    )
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

    if obs.planes.is_empty() {
        content.append(&dim_label(crate::tr_en!("No provenance information available.")));
        return scrolled(&content);
    }

    let multi = obs.planes.len() > 1;
    for (i, plane) in obs.planes.iter().enumerate() {
        let plane_box = gtk::Box::new(gtk::Orientation::Vertical, 12);

        match &plane.provenance {
            None => {
                plane_box.append(&dim_label(crate::tr_en!("No provenance in this plane.")));
            }
            Some(pv) => {
                add_card(
                    &plane_box,
                    crate::tr_en!("Pipeline"),
                    &[
                        (crate::tr_en!("Name").into(), f_text(pv.name.as_deref())),
                        (crate::tr_en!("Version").into(), f_text(pv.version.as_deref())),
                        (crate::tr_en!("Project").into(), f_text(pv.project.as_deref())),
                        (crate::tr_en!("Producer").into(), f_text(pv.producer.as_deref())),
                        (crate::tr_en!("Run ID").into(), f_text(pv.run_id.as_deref())),
                        (crate::tr_en!("Reference").into(), f_text(pv.reference.as_deref())),
                        (crate::tr_en!("Last Executed").into(), f_date(pv.last_executed.as_deref())),
                    ],
                );
                if !pv.inputs.is_empty() {
                    add_inputs_card(&plane_box, &pv.inputs);
                }
            }
        }

        append_plane(&content, plane_box, plane_expander_title(plane), multi, i == 0);
    }

    scrolled(&content)
}

/// Build an "Inputs" card listing the upstream plane URIs that fed a plane's
/// provenance. Mirrors the reference's Inputs heading + per-URI rows.
fn add_inputs_card(container: &gtk::Box, inputs: &[String]) {
    let group = adw::PreferencesGroup::new();
    group.set_title(crate::tr_en!("Inputs"));
    for input in inputs {
        // The URI can contain `&`; disable Pango markup so it renders verbatim.
        let row = adw::ActionRow::builder().title(input.as_str()).build();
        row.set_use_markup(false);
        group.add(&row);
    }
    container.append(&group);
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
    push_raw(&mut lines, "metaRelease", f_date(obs.meta_release.as_deref()));
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
        push_raw(&mut lines, "environment.ambientTemp", f_number(e.ambient_temp));
        push_raw(&mut lines, "environment.photometric", f_bool(e.photometric));
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
        push_raw(&mut lines, &format!("{}energy.bandpass", prefix), f_text(p.energy_bandpass.as_deref()));
        push_raw(&mut lines, &format!("{}energy.lower", prefix), f_wavelength(p.energy_lower));
        push_raw(&mut lines, &format!("{}energy.upper", prefix), f_wavelength(p.energy_upper));
        push_raw(&mut lines, &format!("{}time.lower", prefix), f_mjd_to_date(p.time_lower));
        push_raw(&mut lines, &format!("{}time.upper", prefix), f_mjd_to_date(p.time_upper));
        push_raw(&mut lines, &format!("{}time.exposure", prefix), crate::helpers::caom2_format::seconds(p.time_exposure));
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

/// Activate a named app-level action from a widget (walks up to the window →
/// application action group). Used for `open-fits-file` / `open-cube-file`
/// (STRING path parameter) and `navigate-research` (no parameter) — the same
/// actions the Research page fires.
fn activate_app_action(widget: &gtk::Box, name: &str, param: Option<&glib::Variant>) {
    if let Some(root) = widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        if let Some(app) = root.application() {
            let ag: &gtk::gio::ActionGroup = app.upcast_ref();
            ag.activate_action(name, param);
        }
    }
}

/// Stable, deterministic Research-library id for a publisher DID. Mirrors
/// `search_page::uuid_from_publisher_id` so a download from the detail page shares
/// the managed directory and store slot of a Search-page save (no duplicates).
fn uuid_from_publisher_id(publisher_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    publisher_id.hash(&mut hasher);
    format!("obs-{:016x}", hasher.finish())
}

// ---------------------------------------------------------------------------
// Inline download-status area (per artifact row)
// ---------------------------------------------------------------------------

/// Clear and reveal an artifact row's inline status area.
fn status_reset(status_box: &gtk::Box) {
    while let Some(child) = status_box.first_child() {
        status_box.remove(&child);
    }
    status_box.set_visible(true);
}

/// Show a spinner + label while a download is in flight (matches the page's
/// existing loading-spinner style).
fn status_downloading(status_box: &gtk::Box, text: &str) {
    status_reset(status_box);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    spinner.start();
    row.append(&spinner);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    label.set_xalign(0.0);
    row.append(&label);
    status_box.append(&row);
}

/// Show an inline error line.
fn status_error(status_box: &gtk::Box, text: &str) {
    status_reset(status_box);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("error");
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    label.set_xalign(0.0);
    status_box.append(&label);
}

/// Success state for a downloaded FITS file: a "Downloaded" line, viewer button(s)
/// recommending Cube vs FITS by the sniffed shape, and — when the file was added to
/// the Research library — an "Added to Research" banner with a "View in Research"
/// action. Mirrors the Windows `BuildViewerButton` + `DownloadResearchRow`.
fn status_downloaded_fits(
    status_box: &gtk::Box,
    root: &gtk::Box,
    name: &str,
    path: &str,
    shape: crate::helpers::fits_sniff::FitsShape,
    registered: bool,
) {
    status_reset(status_box);

    let done = gtk::Label::new(Some(&format!("Downloaded {}", name)));
    done.add_css_class("caption");
    done.add_css_class("dim-label");
    done.set_halign(gtk::Align::Start);
    done.set_wrap(true);
    done.set_xalign(0.0);
    status_box.append(&done);

    // Viewer buttons. A pure 2D image → a single FITS-viewer button. A file with a
    // real third axis → BOTH viewers (the recommendation only reorders / styles the
    // buttons, it never restricts the choice).
    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    btn_row.set_halign(gtk::Align::Start);

    let fits_btn = gtk::Button::with_label(crate::tr_en!("Open in FITS Viewer"));
    {
        let root = root.clone();
        let path = path.to_string();
        fits_btn.connect_clicked(move |_| {
            activate_app_action(&root, "open-fits-file", Some(&glib::Variant::from(path.as_str())));
        });
    }

    if shape.has_cube_axis() {
        let cube_btn = gtk::Button::with_label(crate::tr_en!("Open in Cube Viewer"));
        {
            let root = root.clone();
            let path = path.to_string();
            cube_btn.connect_clicked(move |_| {
                activate_app_action(&root, "open-cube-file", Some(&glib::Variant::from(path.as_str())));
            });
        }
        if shape.recommend_cube() {
            // Spectral cube → recommend the Cube Viewer (listed first + accented).
            cube_btn.add_css_class("suggested-action");
            btn_row.append(&cube_btn);
            btn_row.append(&fits_btn);
        } else {
            // Detector stack (3rd axis, but not spectral) → keep the 2D default.
            fits_btn.add_css_class("suggested-action");
            btn_row.append(&fits_btn);
            btn_row.append(&cube_btn);
        }
        status_box.append(&btn_row);
        if shape.recommend_cube() {
            let reco = gtk::Label::new(Some(crate::tr_en!(
                "Spectral cube detected — Cube Viewer recommended"
            )));
            reco.add_css_class("caption");
            reco.add_css_class("accent");
            reco.set_halign(gtk::Align::Start);
            reco.set_xalign(0.0);
            status_box.append(&reco);
        }
    } else {
        fits_btn.add_css_class("suggested-action");
        btn_row.append(&fits_btn);
        status_box.append(&btn_row);
    }

    // "Added to Research" banner with a "View in Research" action.
    if registered {
        let research_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        research_row.set_halign(gtk::Align::Start);
        research_row.set_margin_top(2);
        let added = gtk::Label::new(Some(crate::tr_en!("Added to Research")));
        added.add_css_class("caption");
        added.add_css_class("success");
        added.set_valign(gtk::Align::Center);
        research_row.append(&added);
        let view_btn = gtk::Button::with_label(crate::tr_en!("View in Research"));
        view_btn.add_css_class("flat");
        {
            let root = root.clone();
            view_btn.connect_clicked(move |_| {
                activate_app_action(&root, "navigate-research", None);
            });
        }
        research_row.append(&view_btn);
        status_box.append(&research_row);
    }
}

/// Success state for a non-FITS sidecar (preview PNG / README / …): a "Downloaded"
/// line and an OS-default "Open" button.
fn status_downloaded_other(status_box: &gtk::Box, name: &str, path: &str) {
    status_reset(status_box);

    let done = gtk::Label::new(Some(&format!("Downloaded {}", name)));
    done.add_css_class("caption");
    done.add_css_class("dim-label");
    done.set_halign(gtk::Align::Start);
    done.set_wrap(true);
    done.set_xalign(0.0);
    status_box.append(&done);

    let open_btn = gtk::Button::with_label(crate::tr_en!("Open"));
    open_btn.set_halign(gtk::Align::Start);
    {
        let path = path.to_string();
        open_btn.connect_clicked(move |_| {
            let _ = open::that(&path);
        });
    }
    status_box.append(&open_btn);
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

// ---------------------------------------------------------------------------
// Value formatting (mirrors Helpers/Caom2Format.cs)
// ---------------------------------------------------------------------------

fn f_text(s: Option<&str>) -> String {
    match s {
        Some(v) if !v.trim().is_empty() => v.to_string(),
        _ => DASH.to_string(),
    }
}

/// ISO-8601 timestamp → calendar date `YYYY-MM-DD` (mirrors `Caom2Format.Date`).
/// Tolerant: parses the leading date portion and falls back to the raw text.
fn f_date(s: Option<&str>) -> String {
    match s {
        Some(v) if !v.trim().is_empty() => {
            let t = v.trim();
            let head = t.get(..10).unwrap_or(t);
            match chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d") {
                Ok(d) => d.format("%Y-%m-%d").to_string(),
                Err(_) => t.to_string(),
            }
        }
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
