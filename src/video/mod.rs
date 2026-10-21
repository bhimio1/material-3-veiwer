//! Video playback: libmpv renders each frame into an OpenGL framebuffer
//! inside the egui window (the render API), so videos sit in the same
//! Material layout as pictures, with our own controls.

mod ffi;

use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use ffi::*;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type GetProcAddress = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub selected: bool,
}

/// What the controls show, read from mpv each frame.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub eof: bool,
    pub buffering: bool,
    /// Display size (after aspect ratio), 0 until the first frame.
    pub size: [i64; 2],
    pub codec: String,
    pub fps: f64,
}

pub enum Event {
    Loaded,
    Ended { error: Option<String> },
    Resized,
}

/// A raw pointer that's only used under a lock or from the GL thread.
struct Ptr<T>(*mut T);
// SAFETY: libmpv handles are thread-safe; the render context is only
// touched on the GL thread, under the `Gl` mutex.
unsafe impl<T> Send for Ptr<T> {}
unsafe impl<T> Sync for Ptr<T> {}

struct Gl {
    mpv: Arc<Mpv>,
    render: Ptr<mpv_render_context>,
    fbo: Option<glow::Framebuffer>,
    texture: Option<glow::Texture>,
    size: [i32; 2],
}

pub struct Player {
    mpv: Arc<Mpv>,
    handle: Ptr<mpv_handle>,
    gl: Arc<Mutex<Gl>>,
    // Kept alive for the C callbacks that point at them.
    _repaint: Box<egui::Context>,
    _proc: Box<GetProcAddress>,
    pub state: State,
    pub tracks: Vec<Track>,
    pub file: Option<PathBuf>,
}

unsafe extern "C" fn repaint_cb(ctx: *mut c_void) {
    // SAFETY: points at the Box<egui::Context> the player owns.
    let ctx = unsafe { &*(ctx as *const egui::Context) };
    ctx.request_repaint();
}

unsafe extern "C" fn proc_address_cb(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: points at the Box<GetProcAddress> the player owns; mpv passes
    // a valid C string.
    let get = unsafe { &*(ctx as *const GetProcAddress) };
    get(unsafe { CStr::from_ptr(name) }) as *mut c_void
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Player {
    /// Start mpv and attach it to the window's OpenGL context. Must be
    /// called with that context current (at app creation).
    /// `software_gl`: the window is drawn by a software renderer (llvmpipe
    /// and the like), where mpv's full pipeline is slow and, on some Mesa
    /// versions, draws only black; its simple "dumb mode" works there.
    pub fn new(
        ctx: &egui::Context,
        get_proc_address: GetProcAddress,
        software_gl: bool,
    ) -> Result<Self, String> {
        let mpv = Arc::new(Mpv::load()?);
        // SAFETY: plain libmpv calls on a handle we own; every pointer passed
        // lives until the call returns, or (callbacks) as long as the player.
        unsafe {
            let handle = (mpv.mpv_create)();
            if handle.is_null() {
                return Err("mpv couldn't start".into());
            }
            for (k, v) in [
                ("vo", "libmpv"),
                ("hwdec", "auto-safe"),
                ("keep-open", "yes"),
                ("idle", "yes"),
                ("osc", "no"),
                ("input-default-bindings", "no"),
                ("audio-client-name", "Material 3 Viewer"),
                ("sub-auto", "fuzzy"),
                ("screenshot-format", "png"),
            ] {
                (mpv.mpv_set_option_string)(handle, cstr(k).as_ptr(), cstr(v).as_ptr());
            }
            if software_gl {
                (mpv.mpv_set_option_string)(
                    handle,
                    cstr("gpu-dumb-mode").as_ptr(),
                    cstr("yes").as_ptr(),
                );
            }
            if std::env::var_os("M3_VIEWER_MPV_LOG").is_some() {
                (mpv.mpv_set_option_string)(
                    handle,
                    cstr("terminal").as_ptr(),
                    cstr("yes").as_ptr(),
                );
                (mpv.mpv_set_option_string)(
                    handle,
                    cstr("msg-level").as_ptr(),
                    cstr("all=v").as_ptr(),
                );
            }
            let err = (mpv.mpv_initialize)(handle);
            if err < 0 {
                (mpv.mpv_terminate_destroy)(handle);
                return Err(format!("mpv couldn't start: {}", error_string(&mpv, err)));
            }
            let repaint = Box::new(ctx.clone());
            let repaint_ptr = &*repaint as *const egui::Context as *mut c_void;
            (mpv.mpv_set_wakeup_callback)(handle, Some(repaint_cb), repaint_ptr);

            let proc = Box::new(get_proc_address);
            let mut init = mpv_opengl_init_params {
                get_proc_address: proc_address_cb,
                get_proc_address_ctx: &*proc as *const GetProcAddress as *mut c_void,
            };
            let api = cstr("opengl");
            let mut params = [
                mpv_render_param {
                    kind: RENDER_PARAM_API_TYPE,
                    data: api.as_ptr() as *mut c_void,
                },
                mpv_render_param {
                    kind: RENDER_PARAM_OPENGL_INIT_PARAMS,
                    data: &mut init as *mut _ as *mut c_void,
                },
                mpv_render_param {
                    kind: RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            let mut render = std::ptr::null_mut();
            let err = (mpv.mpv_render_context_create)(&mut render, handle, params.as_mut_ptr());
            if err < 0 {
                (mpv.mpv_terminate_destroy)(handle);
                return Err(format!(
                    "mpv can't draw into this window: {}",
                    error_string(&mpv, err)
                ));
            }
            (mpv.mpv_render_context_set_update_callback)(render, Some(repaint_cb), repaint_ptr);
            Ok(Self {
                gl: Arc::new(Mutex::new(Gl {
                    mpv: mpv.clone(),
                    render: Ptr(render),
                    fbo: None,
                    texture: None,
                    size: [0, 0],
                })),
                mpv,
                handle: Ptr(handle),
                _repaint: repaint,
                _proc: proc,
                state: State {
                    volume: 100.0,
                    speed: 1.0,
                    ..Default::default()
                },
                tracks: Vec::new(),
                file: None,
            })
        }
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: a null-terminated array of valid C strings.
        let err = unsafe { (self.mpv.mpv_command)(self.handle.0, ptrs.as_mut_ptr()) };
        if err < 0 {
            Err(error_string(&self.mpv, err))
        } else {
            Ok(())
        }
    }

    pub fn set(&self, name: &str, value: &str) {
        // SAFETY: valid C strings for the duration of the call.
        unsafe {
            (self.mpv.mpv_set_property_string)(
                self.handle.0,
                cstr(name).as_ptr(),
                cstr(value).as_ptr(),
            );
        }
    }

    pub fn set_f64(&self, name: &str, mut value: f64) {
        // SAFETY: FORMAT_DOUBLE takes a pointer to a double.
        unsafe {
            (self.mpv.mpv_set_property)(
                self.handle.0,
                cstr(name).as_ptr(),
                FORMAT_DOUBLE,
                &mut value as *mut f64 as *mut c_void,
            );
        }
    }

    fn get_f64(&self, name: &str) -> Option<f64> {
        let mut v = 0f64;
        // SAFETY: FORMAT_DOUBLE writes one double.
        let err = unsafe {
            (self.mpv.mpv_get_property)(
                self.handle.0,
                cstr(name).as_ptr(),
                FORMAT_DOUBLE,
                &mut v as *mut f64 as *mut c_void,
            )
        };
        (err >= 0).then_some(v)
    }

    fn get_i64(&self, name: &str) -> Option<i64> {
        let mut v = 0i64;
        // SAFETY: FORMAT_INT64 writes one int64.
        let err = unsafe {
            (self.mpv.mpv_get_property)(
                self.handle.0,
                cstr(name).as_ptr(),
                FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        (err >= 0).then_some(v)
    }

    fn get_bool(&self, name: &str) -> Option<bool> {
        let mut v: c_int = 0;
        // SAFETY: FORMAT_FLAG writes one int.
        let err = unsafe {
            (self.mpv.mpv_get_property)(
                self.handle.0,
                cstr(name).as_ptr(),
                FORMAT_FLAG,
                &mut v as *mut c_int as *mut c_void,
            )
        };
        (err >= 0).then_some(v != 0)
    }

    fn get_string(&self, name: &str) -> Option<String> {
        // SAFETY: mpv returns an owned string (or null) that we free.
        unsafe {
            let p = (self.mpv.mpv_get_property_string)(self.handle.0, cstr(name).as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.mpv.mpv_free)(p as *mut c_void);
            Some(s)
        }
    }

    pub fn load(&mut self, path: &Path, paused: bool) {
        self.file = Some(path.to_path_buf());
        self.tracks.clear();
        self.state.size = [0, 0];
        self.set("pause", if paused { "yes" } else { "no" });
        let _ = self.command(&["loadfile", &path.to_string_lossy(), "replace"]);
    }

    pub fn stop(&mut self) {
        self.file = None;
        self.tracks.clear();
        let _ = self.command(&["stop"]);
    }

    pub fn toggle_pause(&mut self) {
        if self.state.eof {
            // At the end: start over.
            let _ = self.command(&["seek", "0", "absolute"]);
            self.set("pause", "no");
        } else {
            let _ = self.command(&["cycle", "pause"]);
        }
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.command(&["seek", &format!("{seconds:.3}"), "absolute+exact"]);
    }

    pub fn seek_by(&self, seconds: f64) {
        let _ = self.command(&["seek", &format!("{seconds:.3}"), "relative"]);
    }

    pub fn frame_step(&self, forward: bool) {
        let _ = self.command(&[if forward {
            "frame-step"
        } else {
            "frame-back-step"
        }]);
    }

    pub fn set_loop(&self, on: bool) {
        self.set("loop-file", if on { "inf" } else { "no" });
    }

    /// Select a track by kind ("audio", "sub", "video"); `None` turns it off.
    pub fn select_track(&mut self, kind: &str, id: Option<i64>) {
        let prop = match kind {
            "audio" => "aid",
            "sub" => "sid",
            _ => "vid",
        };
        self.set(
            prop,
            &id.map(|i| i.to_string()).unwrap_or_else(|| "no".into()),
        );
        self.read_tracks();
    }

    /// Save the current frame (without subtitles) as a PNG.
    pub fn screenshot(&self, path: &Path) -> Result<(), String> {
        self.command(&["screenshot-to-file", &path.to_string_lossy(), "video"])
    }

    fn read_tracks(&mut self) {
        let count = self.get_i64("track-list/count").unwrap_or(0);
        self.tracks = (0..count)
            .map(|i| {
                let p = |k: &str| {
                    self.get_string(&format!("track-list/{i}/{k}"))
                        .unwrap_or_default()
                };
                let kind = p("type");
                let mut label = p("title");
                let lang = p("lang");
                if !lang.is_empty() {
                    label = if label.is_empty() {
                        lang
                    } else {
                        format!("{label} ({lang})")
                    };
                }
                if label.is_empty() {
                    label = format!("Track {}", p("id"));
                }
                let codec = p("codec");
                if !codec.is_empty() && kind != "video" {
                    label = format!("{label} · {codec}");
                }
                Track {
                    id: p("id").parse().unwrap_or(0),
                    kind,
                    label,
                    selected: p("selected") == "yes",
                }
            })
            .collect();
    }

    /// Handle mpv's events and refresh [`Player::state`]. Call every frame.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            // SAFETY: mpv_wait_event returns a pointer valid until the next
            // call; we copy what we need out of it right away.
            let (id, error, end_error) = unsafe {
                let ev = &*(self.mpv.mpv_wait_event)(self.handle.0, 0.0);
                let end_error = if ev.event_id == EVENT_END_FILE && !ev.data.is_null() {
                    let end = &*(ev.data as *const mpv_event_end_file);
                    // 4 = MPV_END_FILE_REASON_ERROR
                    (end.reason == 4).then_some(end.error)
                } else {
                    None
                };
                (ev.event_id, ev.error, end_error)
            };
            match id {
                EVENT_NONE | EVENT_SHUTDOWN => break,
                EVENT_FILE_LOADED => {
                    self.read_tracks();
                    events.push(Event::Loaded);
                }
                EVENT_END_FILE => events.push(Event::Ended {
                    error: end_error
                        .map(|e| error_string(&self.mpv, if e < 0 { e } else { error })),
                }),
                EVENT_VIDEO_RECONFIG => {
                    self.state.size = [
                        self.get_i64("dwidth").unwrap_or(0),
                        self.get_i64("dheight").unwrap_or(0),
                    ];
                    events.push(Event::Resized);
                }
                _ => {}
            }
        }
        if self.file.is_some() {
            self.state.position = self.get_f64("time-pos").unwrap_or(0.0);
            self.state.duration = self.get_f64("duration").unwrap_or(0.0);
            self.state.eof = self.get_bool("eof-reached").unwrap_or(false);
            self.state.buffering = self.get_bool("paused-for-cache").unwrap_or(false);
            self.state.fps = self.get_f64("container-fps").unwrap_or(0.0);
            self.state.codec = self.get_string("video-codec").unwrap_or_default();
        }
        self.state.paused = self.get_bool("pause").unwrap_or(true);
        self.state.volume = self.get_f64("volume").unwrap_or(100.0);
        self.state.muted = self.get_bool("mute").unwrap_or(false);
        self.state.speed = self.get_f64("speed").unwrap_or(1.0);
        events
    }

    /// Draws the current video frame into `rect`.
    pub fn paint_callback(&self, rect: egui::Rect) -> egui::PaintCallback {
        let gl = self.gl.clone();
        egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                if let Ok(mut gl_state) = gl.lock() {
                    gl_state.render(painter, &info);
                }
            })),
        }
    }

    /// Free the GL side. Call with the window's GL context current
    /// (`App::on_exit`); after this the player can't draw.
    pub fn shutdown_gl(&mut self, gl: Option<&glow::Context>) {
        if let Ok(mut state) = self.gl.lock() {
            if !state.render.0.is_null() {
                // SAFETY: called on the GL thread with the context current.
                unsafe { (self.mpv.mpv_render_context_free)(state.render.0) };
                state.render.0 = std::ptr::null_mut();
            }
            if let Some(gl) = gl {
                // SAFETY: deleting objects we created on this context.
                unsafe {
                    if let Some(f) = state.fbo.take() {
                        gl.delete_framebuffer(f);
                    }
                    if let Some(t) = state.texture.take() {
                        gl.delete_texture(t);
                    }
                }
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let render_alive = self
            .gl
            .lock()
            .map(|g| !g.render.0.is_null())
            .unwrap_or(true);
        // mpv waits for the render context to go away before it shuts down,
        // which needs the GL context; if that never happened, leave mpv to
        // the OS rather than hang on exit.
        if !render_alive {
            // SAFETY: the handle is ours and nothing uses it after this.
            unsafe { (self.mpv.mpv_terminate_destroy)(self.handle.0) };
        }
    }
}

impl Gl {
    fn render(&mut self, painter: &egui_glow::Painter, info: &egui::PaintCallbackInfo) {
        if self.render.0.is_null() {
            return;
        }
        let vp = info.viewport_in_pixels();
        let (w, h) = (vp.width_px, vp.height_px);
        if w <= 0 || h <= 0 {
            return;
        }
        let gl = painter.gl();
        // SAFETY: GL calls on the current context from egui's paint callback;
        // the framebuffer and texture are ours.
        unsafe {
            if self.fbo.is_none() || self.size != [w, h] {
                let texture = match self.texture {
                    Some(t) => t,
                    None => match gl.create_texture() {
                        Ok(t) => t,
                        Err(_) => return,
                    },
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    w,
                    h,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                let fbo = match self.fbo {
                    Some(f) => f,
                    None => match gl.create_framebuffer() {
                        Ok(f) => f,
                        Err(_) => return,
                    },
                };
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                self.texture = Some(texture);
                self.fbo = Some(fbo);
                self.size = [w, h];
            }
            let Some(fbo) = self.fbo else { return };
            let mut target = mpv_opengl_fbo {
                fbo: fbo.0.get() as c_int,
                w,
                h,
                internal_format: 0,
            };
            let mut flip: c_int = 1;
            let mut block: c_int = 0;
            let mut params = [
                mpv_render_param {
                    kind: RENDER_PARAM_OPENGL_FBO,
                    data: &mut target as *mut _ as *mut c_void,
                },
                mpv_render_param {
                    kind: RENDER_PARAM_FLIP_Y,
                    data: &mut flip as *mut _ as *mut c_void,
                },
                // Never stall the UI waiting for a frame's display time.
                mpv_render_param {
                    kind: RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                    data: &mut block as *mut _ as *mut c_void,
                },
                mpv_render_param {
                    kind: RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            // Hand mpv a clean GL state: egui leaves blending, scissoring and
            // its own program and buffers bound.
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.use_program(None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.viewport(0, 0, w, h);
            // mpv does its frame bookkeeping here; required before each render.
            (self.mpv.mpv_render_context_update)(self.render.0);
            (self.mpv.mpv_render_context_render)(self.render.0, params.as_mut_ptr());
            // Copy the frame into the window where egui reserved room for it.
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, painter.intermediate_fbo());
            gl.enable(glow::SCISSOR_TEST);
            let clip = clip_in_pixels(info);
            gl.scissor(clip[0], clip[1], clip[2], clip[3]);
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                vp.left_px,
                vp.from_bottom_px,
                vp.left_px + w,
                vp.from_bottom_px + h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
        }
    }
}

/// The callback's clip rectangle as a GL scissor box.
fn clip_in_pixels(info: &egui::PaintCallbackInfo) -> [i32; 4] {
    let c = info.clip_rect_in_pixels();
    [c.left_px, c.from_bottom_px, c.width_px, c.height_px]
}

fn error_string(mpv: &Mpv, err: c_int) -> String {
    // SAFETY: mpv_error_string returns a static string.
    unsafe {
        let p = (mpv.mpv_error_string)(err);
        if p.is_null() {
            format!("error {err}")
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// `setlocale(LC_NUMERIC, "C")`: mpv refuses to start unless numbers are
/// formatted the C way. Call at the start of `main`, before other threads.
pub fn init_locale() {
    extern "C" {
        fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    }
    const LC_NUMERIC: c_int = 1;
    // SAFETY: a static C string; called once while starting up.
    unsafe {
        setlocale(LC_NUMERIC, c"C".as_ptr());
    }
}

/// "1:05", "1:02:03".
pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.4), "1:05");
        assert_eq!(format_time(3723.0), "1:02:03");
        assert_eq!(format_time(-3.0), "0:00");
    }
}
