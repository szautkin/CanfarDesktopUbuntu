//! GPU volume ray-marcher bound to a `gtk::GLArea`.
//!
//! Port of `Services/CubeViewer/CubeVolumeRenderer.cs` (D3D11) to OpenGL 3.3 core.
//! The GLSL is in `helpers::cube_gl_shaders`. This widget owns the GL objects,
//! the camera (orbit/zoom/auto-orbit), and the render parameters (window, stretch,
//! colormap, density, transfer ramp, MIP).
//!
//! Robustness: GL init and shader compilation are fallible (software GL on
//! headless/llvmpipe, missing GL 3.2). On any failure the widget sets a `failed`
//! flag and simply clears — the parent page then falls back to slice-only mode.

use crate::helpers::cube_gl_shaders::{fragment_src, vertex_src};
use crate::helpers::cube_math::{self, Mat4};
use crate::helpers::cube_slice::StretchMode;
use crate::models::volume_data::VolumeData;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;
use std::sync::Once;

/// Load the GL function pointers exactly once via libepoxy.
fn ensure_gl_loaded() {
    static LOAD: Once = Once::new();
    LOAD.call_once(|| {
        #[cfg(all(unix, not(target_os = "macos")))]
        let library = unsafe { libloading::Library::new("libepoxy.so.0") }
            .or_else(|_| unsafe { libloading::Library::new("libepoxy.so") });
        #[cfg(target_os = "macos")]
        let library = unsafe { libloading::Library::new("libepoxy.0.dylib") };
        if let Ok(library) = library {
            epoxy::load_with(|name| {
                unsafe { library.get::<*const std::ffi::c_void>(name.as_bytes()) }
                    .map(|sym| *sym)
                    .unwrap_or(std::ptr::null())
            });
            gl::load_with(|name| epoxy::get_proc_addr(name));
            // Keep libepoxy resident for the process lifetime.
            std::mem::forget(library);
        }
    });
}

#[derive(Default)]
struct GlState {
    realized: bool,
    failed: bool,
    /// The context is OpenGL ES (GLES shader dialect + ES-safe texture params).
    is_es: bool,
    program: u32,
    vao: u32,
    data_tex: u32,
    cmap_tex: u32,
    tf_tex: u32,
    // uniform locations
    u_inv_vp: i32,
    u_inv_model: i32,
    u_window: i32,
    u_steps: i32,
    u_density: i32,
    u_jitter: i32,
    u_stretch: i32,
    u_mip: i32,
    u_data: i32,
    u_cmap: i32,
    u_tf: i32,
    // pending GPU uploads
    volume: Option<VolumeData>,
    volume_dirty: bool,
    cmap_lut: Vec<u8>,
    cmap_dirty: bool,
    tf_ramp: Vec<u8>,
    tf_dirty: bool,
    // render parameters
    window: (f32, f32),
    stretch: i32,
    density: f32,
    steps: f32, // base steps
    mip: i32,
    jitter: f32,
    /// True while the user is actively orbiting (drops step count for fluid motion).
    interacting: bool,
    /// Clear/background colour (RGBA).
    bg_color: [f32; 4],
    // native spatial dims (for the model aspect); spectral axis uses spectral_scale.
    vol_nx: usize,
    vol_ny: usize,
    spectral_scale: f32,
    // camera
    az: f32,
    el: f32,
    dist: f32,
    auto_orbit: bool,
}

pub struct CubeVolumeGl {
    area: gtk::GLArea,
    state: Rc<RefCell<GlState>>,
    /// Invoked whenever the camera moves, so the axes overlay can redraw in sync.
    camera_cb: Rc<RefCell<Option<Box<dyn Fn()>>>>,
}

impl CubeVolumeGl {
    pub fn new() -> Rc<Self> {
        let area = gtk::GLArea::new();
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(false);
        area.set_auto_render(true);
        area.set_hexpand(true);
        area.set_vexpand(true);
        // Do NOT pin a desktop-GL version here: on some drivers (NVIDIA over
        // EGL) GDK's display context is OpenGL **ES**, and a required 3.3
        // desktop context can never be created ("Unable to create a GL
        // context"). Accept whatever GDK negotiates — desktop GL 3.3+ core on
        // most systems, GLES 3.x otherwise — and pick the shader dialect at
        // realize time.

        let mut init = GlState {
            window: (0.0, 1.0),
            stretch: StretchMode::Linear as i32,
            density: 1.0,
            steps: 384.0, // BaseSteps (matches CubeVolumeRenderer)
            mip: 0,
            jitter: 0.0,
            interacting: false,
            bg_color: [0.06, 0.06, 0.08, 1.0],
            cmap_lut: crate::helpers::cube_colormaps::lut_rgba(
                crate::helpers::cube_colormaps::DEFAULT,
            ),
            cmap_dirty: true,
            tf_ramp: crate::helpers::transfer_function::TransferFunctionModel::default_ramp()
                .ramp()
                .to_vec(),
            tf_dirty: true,
            vol_nx: 1,
            vol_ny: 1,
            spectral_scale: 1.5,
            az: 0.7,
            el: 0.5,
            dist: 2.6,
            ..Default::default()
        };
        // StretchMode -> shader index mapping is guaranteed by the enum order.
        init.stretch = stretch_index(StretchMode::Linear);

        let state = Rc::new(RefCell::new(init));

        let this = Rc::new(CubeVolumeGl {
            area: area.clone(),
            state: state.clone(),
            camera_cb: Rc::new(RefCell::new(None)),
        });

        // Realize: load GL, compile the program, create GL objects.
        {
            let state = state.clone();
            area.connect_realize(move |area| {
                area.make_current();
                if let Some(err) = area.error() {
                    eprintln!("[cube-gl] GLArea realize error (falling back to slice mode): {err}");
                    state.borrow_mut().failed = true;
                    return;
                }
                ensure_gl_loaded();
                unsafe {
                    let mut is_es = false;
                    let ver = gl::GetString(gl::VERSION);
                    if !ver.is_null() {
                        let ver = std::ffi::CStr::from_ptr(ver as *const _).to_string_lossy();
                        is_es = ver.starts_with("OpenGL ES");
                        eprintln!("[cube-gl] OpenGL context: {ver}");
                    }
                    let mut s = state.borrow_mut();
                    s.is_es = is_es;
                    realize_gl(&mut s);
                }
            });
        }
        // Unrealize: free GL objects.
        {
            let state = state.clone();
            area.connect_unrealize(move |area| {
                area.make_current();
                unsafe { unrealize_gl(&mut state.borrow_mut()) };
            });
        }
        // Render.
        {
            let state = state.clone();
            let area_for_size = area.clone();
            area.connect_render(move |_area, _ctx| {
                // The GL framebuffer is in DEVICE pixels: on HiDPI (scale 2) it is
                // twice the logical widget size. A viewport set from logical
                // width()/height() would render into the bottom-left quarter —
                // visibly "outside" the cairo axes overlay, which draws in
                // logical coordinates over the full area.
                let sf = area_for_size.scale_factor().max(1);
                let w = area_for_size.width().max(1) * sf;
                let h = area_for_size.height().max(1) * sf;
                unsafe { render_gl(&mut state.borrow_mut(), w, h) };
                glib::Propagation::Stop
            });
        }

        this.setup_interaction();
        this
    }

    pub fn widget(&self) -> &gtk::GLArea {
        &self.area
    }

    /// True when the GL context + shaders initialised successfully.
    pub fn is_available(&self) -> bool {
        let s = self.state.borrow();
        s.realized && !s.failed
    }

    pub fn set_volume(&self, vol: VolumeData) {
        let mut s = self.state.borrow_mut();
        // Model X/Y use the spatial aspect (max of the two spatial dims); the
        // spectral Z axis is stretched by `spectral_scale` (matches CubeVolumeRenderer).
        s.vol_nx = vol.nx.max(1);
        s.vol_ny = vol.ny.max(1);
        s.volume = Some(vol);
        s.volume_dirty = true;
        drop(s);
        self.area.queue_render();
    }

    pub fn set_colormap(&self, name: &str) {
        {
            let mut s = self.state.borrow_mut();
            s.cmap_lut = crate::helpers::cube_colormaps::lut_rgba(name);
            s.cmap_dirty = true;
        }
        self.area.queue_render();
    }

    pub fn set_window(&self, lo: f32, hi: f32) {
        self.state.borrow_mut().window = (lo, hi);
        self.area.queue_render();
    }

    pub fn set_stretch(&self, mode: StretchMode) {
        self.state.borrow_mut().stretch = stretch_index(mode);
        self.area.queue_render();
    }

    pub fn set_density(&self, d: f32) {
        self.state.borrow_mut().density = d.max(0.01);
        self.area.queue_render();
    }

    pub fn set_steps(&self, steps: f32) {
        self.state.borrow_mut().steps = steps.clamp(32.0, 1024.0);
        self.area.queue_render();
    }

    pub fn set_mip(&self, on: bool) {
        self.state.borrow_mut().mip = if on { 1 } else { 0 };
        self.area.queue_render();
    }

    pub fn set_transfer_ramp(&self, ramp: [u8; 256]) {
        {
            let mut s = self.state.borrow_mut();
            s.tf_ramp = ramp.to_vec();
            s.tf_dirty = true;
        }
        self.area.queue_render();
    }

    pub fn set_auto_orbit(self: &Rc<Self>, on: bool) {
        self.state.borrow_mut().auto_orbit = on;
        if on {
            let this = self.clone();
            // ~60 fps auto-orbit; pauses while dragging, stops when cleared.
            glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                let mut s = this.state.borrow_mut();
                if !s.auto_orbit {
                    return glib::ControlFlow::Break;
                }
                if !s.interacting {
                    s.az += 0.0016;
                }
                drop(s);
                this.area.queue_render();
                this.fire_camera_changed();
                glib::ControlFlow::Continue
            });
        }
    }

    fn fire_camera_changed(&self) {
        if let Some(cb) = self.camera_cb.borrow().as_ref() {
            cb();
        }
    }

    /// Register a callback fired whenever the camera moves (for the axes overlay).
    pub fn set_on_camera_changed(&self, cb: impl Fn() + 'static) {
        *self.camera_cb.borrow_mut() = Some(Box::new(cb));
    }

    fn setup_interaction(self: &Rc<Self>) {
        // Drag to orbit (matches CubeViewerViewModel.Orbit: az -= dx, el += dy).
        let drag = gtk::GestureDrag::new();
        {
            let this = self.clone();
            let start = Rc::new(RefCell::new((0.0f32, 0.0f32)));
            let s0 = start.clone();
            let t0 = this.clone();
            drag.connect_drag_begin(move |_, _, _| {
                let mut s = t0.state.borrow_mut();
                s.interacting = true;
                *s0.borrow_mut() = (s.az, s.el);
            });
            let s0 = start;
            let t1 = this.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let (az0, el0) = *s0.borrow();
                {
                    let mut s = t1.state.borrow_mut();
                    s.az = az0 - dx as f32 * 0.01;
                    s.el = (el0 + dy as f32 * 0.01).clamp(-1.4, 1.4);
                }
                t1.area.queue_render();
                t1.fire_camera_changed();
            });
            let t2 = this.clone();
            drag.connect_drag_end(move |_, _, _| {
                t2.state.borrow_mut().interacting = false;
                t2.area.queue_render();
            });
        }
        self.area.add_controller(drag);

        // Scroll to zoom (dolly): dist *= exp(delta), clamp [0.5, 8].
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let this = self.clone();
            scroll.connect_scroll(move |_, _dx, dy| {
                {
                    let mut s = this.state.borrow_mut();
                    s.dist = (s.dist * (dy as f32 * 0.12).exp()).clamp(0.5, 8.0);
                }
                this.area.queue_render();
                this.fire_camera_changed();
                glib::Propagation::Stop
            });
        }
        self.area.add_controller(scroll);
    }

    /// The camera view-projection (no model scale) for the axes overlay.
    pub fn view_proj(&self, w: i32, h: i32) -> Mat4 {
        let s = self.state.borrow();
        let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };
        view_proj_of(&s, aspect)
    }

    pub fn spectral_scale(&self) -> f32 {
        self.state.borrow().spectral_scale
    }

    pub fn set_spectral_scale(&self, v: f32) {
        self.state.borrow_mut().spectral_scale = v.clamp(0.5, 4.0);
        self.area.queue_render();
        self.fire_camera_changed();
    }

    /// Set the clear/background colour (RGB in 0..1).
    pub fn set_background(&self, rgb: [f32; 3]) {
        self.state.borrow_mut().bg_color = [rgb[0], rgb[1], rgb[2], 1.0];
        self.area.queue_render();
    }

    /// Reset the camera to the default framing.
    pub fn reset_view(&self) {
        {
            let mut s = self.state.borrow_mut();
            s.az = 0.7;
            s.el = 0.5;
            s.dist = 2.6;
        }
        self.area.queue_render();
        self.fire_camera_changed();
    }

    /// Current orbit camera as `(azimuth, elevation, distance)` — for live MCP
    /// readout of the 3D view.
    pub fn camera(&self) -> (f32, f32, f32) {
        let s = self.state.borrow();
        (s.az, s.el, s.dist)
    }

    /// Set the orbit camera, clamping elevation/distance exactly like the
    /// interactive drag/zoom paths, then repaint + notify the overlay.
    pub fn set_camera(&self, az: f32, el: f32, dist: f32) {
        {
            let mut s = self.state.borrow_mut();
            s.az = az;
            s.el = el.clamp(-1.4, 1.4);
            s.dist = dist.clamp(0.5, 8.0);
        }
        self.area.queue_render();
        self.fire_camera_changed();
    }

    /// Current base ray-march step budget (quality).
    pub fn steps(&self) -> f32 {
        self.state.borrow().steps
    }

    /// Render the current view offscreen and read it back as straight RGBA8
    /// (top-down). `transparent` clears to alpha 0 for figure export onto a plate.
    /// Returns `None` if GL isn't available.
    pub fn render_to_rgba(&self, width: i32, height: i32, transparent: bool) -> Option<Vec<u8>> {
        {
            let s = self.state.borrow();
            if !s.realized || s.failed || s.program == 0 {
                return None;
            }
        }
        self.area.make_current();
        if self.area.error().is_some() {
            return None;
        }
        let w = width.max(1);
        let h = height.max(1);
        let mut s = self.state.borrow_mut();
        unsafe {
            let mut tex = 0u32;
            gl::GenTextures(1, &mut tex);
            gl::BindTexture(gl::TEXTURE_2D, tex);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                w,
                h,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
            let mut fbo = 0u32;
            gl::GenFramebuffers(1, &mut fbo);
            gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
            gl::FramebufferTexture2D(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                tex,
                0,
            );
            let mut pixels: Option<Vec<u8>> = None;
            if gl::CheckFramebufferStatus(gl::FRAMEBUFFER) == gl::FRAMEBUFFER_COMPLETE {
                gl::Viewport(0, 0, w, h);
                if transparent {
                    gl::ClearColor(0.0, 0.0, 0.0, 0.0);
                } else {
                    gl::ClearColor(s.bg_color[0], s.bg_color[1], s.bg_color[2], 1.0);
                }
                gl::Clear(gl::COLOR_BUFFER_BIT);
                ensure_uploads(&mut s);
                let aspect = w as f32 / h as f32;
                draw_scene(&s, aspect, s.steps, s.jitter);
                let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
                gl::PixelStorei(gl::PACK_ALIGNMENT, 1);
                gl::ReadPixels(
                    0,
                    0,
                    w,
                    h,
                    gl::RGBA,
                    gl::UNSIGNED_BYTE,
                    buf.as_mut_ptr() as *mut _,
                );
                // GL reads bottom-up; flip to a top-down image.
                let row = (w as usize) * 4;
                let mut flipped = vec![0u8; buf.len()];
                for y in 0..h as usize {
                    let src = (h as usize - 1 - y) * row;
                    flipped[y * row..y * row + row].copy_from_slice(&buf[src..src + row]);
                }
                pixels = Some(flipped);
            }
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
            gl::DeleteFramebuffers(1, &fbo);
            gl::DeleteTextures(1, &tex);
            pixels
        }
    }
}

fn stretch_index(mode: StretchMode) -> i32 {
    match mode {
        StretchMode::Linear => 0,
        StretchMode::Log => 1,
        StretchMode::Sqrt => 2,
        StretchMode::Squared => 3,
        StretchMode::Asinh => 4,
    }
}

// ── GL helpers (unsafe) ─────────────────────────────────────────────────────

unsafe fn compile_shader(kind: u32, src: &str) -> Result<u32, String> {
    let shader = gl::CreateShader(kind);
    let c = CString::new(src).unwrap();
    gl::ShaderSource(shader, 1, &c.as_ptr(), std::ptr::null());
    gl::CompileShader(shader);
    let mut ok = 0i32;
    gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut len = 0i32;
        gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len);
        let mut buf = vec![0u8; len.max(1) as usize];
        gl::GetShaderInfoLog(
            shader,
            len,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
        );
        gl::DeleteShader(shader);
        return Err(String::from_utf8_lossy(&buf).into_owned());
    }
    Ok(shader)
}

unsafe fn realize_gl(s: &mut GlState) {
    let vs = match compile_shader(gl::VERTEX_SHADER, &vertex_src(s.is_es)) {
        Ok(v) => v,
        Err(log) => {
            eprintln!("[cube-gl] vertex shader failed (slice fallback): {log}");
            s.failed = true;
            return;
        }
    };
    let fs = match compile_shader(gl::FRAGMENT_SHADER, &fragment_src(s.is_es)) {
        Ok(v) => v,
        Err(log) => {
            eprintln!("[cube-gl] fragment shader failed (slice fallback): {log}");
            gl::DeleteShader(vs);
            s.failed = true;
            return;
        }
    };
    let program = gl::CreateProgram();
    gl::AttachShader(program, vs);
    gl::AttachShader(program, fs);
    gl::LinkProgram(program);
    gl::DeleteShader(vs);
    gl::DeleteShader(fs);
    let mut ok = 0i32;
    gl::GetProgramiv(program, gl::LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut len = 0i32;
        gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len);
        let mut buf = vec![0u8; len.max(1) as usize];
        gl::GetProgramInfoLog(
            program,
            len,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
        );
        eprintln!(
            "[cube-gl] program link failed (slice fallback): {}",
            String::from_utf8_lossy(&buf)
        );
        gl::DeleteProgram(program);
        s.failed = true;
        return;
    }
    s.program = program;

    // A VAO is required in core profile even for attribute-less draws.
    gl::GenVertexArrays(1, &mut s.vao);
    gl::GenTextures(1, &mut s.data_tex);
    gl::GenTextures(1, &mut s.cmap_tex);
    gl::GenTextures(1, &mut s.tf_tex);

    let loc = |name: &str| {
        let c = CString::new(name).unwrap();
        gl::GetUniformLocation(s.program, c.as_ptr())
    };
    s.u_inv_vp = loc("invViewProj");
    s.u_inv_model = loc("inverseModel");
    s.u_window = loc("window");
    s.u_steps = loc("steps");
    s.u_density = loc("density");
    s.u_jitter = loc("jitter");
    s.u_stretch = loc("stretch");
    s.u_mip = loc("mip");
    s.u_data = loc("dataTex");
    s.u_cmap = loc("cmapTex");
    s.u_tf = loc("tfTex");

    s.realized = true;
}

unsafe fn unrealize_gl(s: &mut GlState) {
    if s.program != 0 {
        gl::DeleteProgram(s.program);
        s.program = 0;
    }
    if s.vao != 0 {
        gl::DeleteVertexArrays(1, &s.vao);
        s.vao = 0;
    }
    for t in [s.data_tex, s.cmap_tex, s.tf_tex] {
        if t != 0 {
            gl::DeleteTextures(1, &t);
        }
    }
    s.data_tex = 0;
    s.cmap_tex = 0;
    s.tf_tex = 0;
    s.realized = false;
}

unsafe fn upload_1d_rgba(tex: u32, unit: u32, rgba: &[u8]) {
    let width = (rgba.len() / 4) as i32;
    gl::ActiveTexture(gl::TEXTURE0 + unit);
    gl::BindTexture(gl::TEXTURE_2D, tex);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
    gl::TexImage2D(
        gl::TEXTURE_2D,
        0,
        gl::RGBA8 as i32,
        width,
        1,
        0,
        gl::RGBA,
        gl::UNSIGNED_BYTE,
        rgba.as_ptr() as *const _,
    );
}

/// `view_proj = perspective(38°) * look_at(orbit)`, WITHOUT the box/model scale
/// (the axes overlay applies the same box scale itself, matching CubeAxesOverlay).
fn view_proj_of(s: &GlState, aspect: f32) -> Mat4 {
    let eye = cube_math::orbit_eye(s.az, s.el, s.dist, [0.0, 0.0, 0.0]);
    let view = cube_math::look_at(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let proj = cube_math::perspective(38.0f32.to_radians(), aspect, 0.01, 50.0);
    cube_math::mul(&proj, &view)
}

/// Model = Scale(nx/m, ny/m, spectral_scale), m = max(nx,ny) — spatial aspect in
/// X/Y, user-controlled spectral stretch in Z (matches CubeVolumeRenderer).
fn model_of(s: &GlState) -> Mat4 {
    let m = s.vol_nx.max(s.vol_ny).max(1) as f32;
    cube_math::scale(s.vol_nx as f32 / m, s.vol_ny as f32 / m, s.spectral_scale)
}

unsafe fn ensure_uploads(s: &mut GlState) {
    if s.volume_dirty {
        if let Some(vol) = &s.volume {
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_3D, s.data_tex);
            gl::TexParameteri(gl::TEXTURE_3D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_3D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            // CLAMP_TO_BORDER (+ border colour) is desktop-GL / GLES 3.2-only;
            // use edge clamping on ES so ES 3.0/3.1 contexts stay valid — the
            // ray-march samples inside the unit cube, so the difference is
            // invisible in practice.
            let wrap = if s.is_es {
                gl::CLAMP_TO_EDGE
            } else {
                gl::CLAMP_TO_BORDER
            };
            gl::TexParameteri(gl::TEXTURE_3D, gl::TEXTURE_WRAP_S, wrap as i32);
            gl::TexParameteri(gl::TEXTURE_3D, gl::TEXTURE_WRAP_T, wrap as i32);
            gl::TexParameteri(gl::TEXTURE_3D, gl::TEXTURE_WRAP_R, wrap as i32);
            if !s.is_es {
                let border = [0.0f32, 0.0, 0.0, 0.0];
                gl::TexParameterfv(gl::TEXTURE_3D, gl::TEXTURE_BORDER_COLOR, border.as_ptr());
            }
            gl::PixelStorei(gl::UNPACK_ALIGNMENT, 1);
            gl::TexImage3D(
                gl::TEXTURE_3D,
                0,
                gl::R32F as i32,
                vol.nx as i32,
                vol.ny as i32,
                vol.nz as i32,
                0,
                gl::RED,
                gl::FLOAT,
                vol.data.as_ptr() as *const _,
            );
        }
        s.volume_dirty = false;
    }
    if s.cmap_dirty {
        upload_1d_rgba(s.cmap_tex, 1, &s.cmap_lut);
        s.cmap_dirty = false;
    }
    if s.tf_dirty {
        let mut rgba = vec![0u8; 256 * 4];
        for (i, &a) in s.tf_ramp.iter().enumerate() {
            rgba[i * 4] = a;
        }
        upload_1d_rgba(s.tf_tex, 2, &rgba);
        s.tf_dirty = false;
    }
}

/// Bind the program + textures and march the volume into the currently-bound
/// framebuffer/viewport. Shared by the live present path and the offscreen export.
unsafe fn draw_scene(s: &GlState, aspect: f32, steps: f32, jitter: f32) {
    let view_proj = view_proj_of(s, aspect);
    let inv_vp: Mat4 = cube_math::invert(&view_proj).unwrap_or_else(cube_math::identity);
    let model = model_of(s);
    let inv_model = cube_math::invert(&model).unwrap_or_else(cube_math::identity);

    gl::UseProgram(s.program);
    gl::BindVertexArray(s.vao);
    gl::Disable(gl::CULL_FACE);
    gl::Disable(gl::DEPTH_TEST);
    gl::Enable(gl::BLEND);
    gl::BlendFunc(gl::ONE, gl::ONE_MINUS_SRC_ALPHA); // premultiplied over

    gl::UniformMatrix4fv(s.u_inv_vp, 1, gl::FALSE, inv_vp.as_ptr());
    gl::UniformMatrix4fv(s.u_inv_model, 1, gl::FALSE, inv_model.as_ptr());
    gl::Uniform2f(s.u_window, s.window.0, s.window.1);
    gl::Uniform1f(s.u_steps, steps);
    gl::Uniform1f(s.u_density, s.density);
    gl::Uniform1f(s.u_jitter, jitter);
    gl::Uniform1i(s.u_stretch, s.stretch);
    gl::Uniform1i(s.u_mip, s.mip);

    gl::ActiveTexture(gl::TEXTURE0);
    gl::BindTexture(gl::TEXTURE_3D, s.data_tex);
    gl::Uniform1i(s.u_data, 0);
    gl::ActiveTexture(gl::TEXTURE1);
    gl::BindTexture(gl::TEXTURE_2D, s.cmap_tex);
    gl::Uniform1i(s.u_cmap, 1);
    gl::ActiveTexture(gl::TEXTURE2);
    gl::BindTexture(gl::TEXTURE_2D, s.tf_tex);
    gl::Uniform1i(s.u_tf, 2);

    gl::DrawArrays(gl::TRIANGLES, 0, 3);

    gl::BindVertexArray(0);
    gl::UseProgram(0);
}

unsafe fn render_gl(s: &mut GlState, w: i32, h: i32) {
    gl::Viewport(0, 0, w, h);
    gl::ClearColor(s.bg_color[0], s.bg_color[1], s.bg_color[2], s.bg_color[3]);
    gl::Clear(gl::COLOR_BUFFER_BIT);
    if s.failed || s.program == 0 {
        return;
    }
    ensure_uploads(s);
    // Drop steps while orbiting for fluid motion; animate jitter so banding
    // dissolves across frames.
    let steps = if s.interacting {
        s.steps.min(160.0)
    } else {
        s.steps
    };
    s.jitter = (s.jitter + 17.13) % 1024.0;
    let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };
    let jitter = s.jitter;
    draw_scene(s, aspect, steps, jitter);
}
