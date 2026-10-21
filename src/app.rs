//! The viewer window: state, navigation, keyboard, and background jobs.
//! Drawing lives in `ui/`.

use crate::config::Config;
use crate::edit::{Aspect, Crop, Edits};
use crate::loader::{Loader, Picture, Slot};
use crate::media::{self, Kind};
use crate::video::{self, Player};
use egui::{Key, Vec2};
use material_3_ui::sys::task::Task;
use material_3_ui::theme::ThemeManager;
use material_3_ui::toasts::Toasts;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    None,
    Info,
    Edit,
    Settings,
}

/// What a background job reports back.
pub enum Done {
    Message(Result<String, String>),
    Opened(Option<PathBuf>),
    /// An edited image was written to this path.
    Saved(Result<PathBuf, String>),
    Trashed(PathBuf, Result<(), String>),
}

#[derive(Clone, PartialEq)]
pub struct PreviewKey {
    pub path: PathBuf,
    pub edits: Edits,
    pub with_crop: bool,
}

pub struct Preview {
    pub key: PreviewKey,
    pub texture: egui::TextureHandle,
}

pub struct CropState {
    pub aspect: Aspect,
    pub draft: Crop,
}

/// Modal dialogs.
#[derive(Clone, PartialEq)]
pub enum Confirm {
    Trash(PathBuf),
    Overwrite(PathBuf),
}

pub struct ViewerApp {
    pub cfg: Config,
    pub theme: ThemeManager,
    pub toasts: Toasts<()>,
    pub loader: Loader,
    pub player: Option<Player>,
    pub player_error: Option<String>,

    pub files: Vec<PathBuf>,
    pub index: usize,

    /// Image pixels per physical pixel; `None` follows the default zoom.
    pub zoom: Option<f32>,
    pub offset: Vec2,
    pub anim_start: Instant,
    pub fullscreen: bool,
    pub sheet: Sheet,
    pub slideshow: Option<Instant>,
    pub last_activity: Instant,
    pub scroll_filmstrip: bool,
    /// A zoom step from the keyboard, applied by the canvas (which knows
    /// the fit scale).
    pub pending_zoom: Option<f32>,
    /// The zoom on screen last frame (1.0 = actual pixels), for the label.
    pub shown_zoom: f32,
    /// Whether the video controls were on screen last frame.
    pub controls_shown: bool,

    pub edits: Edits,
    pub crop: Option<CropState>,
    pub resize_text: [String; 2],
    pub resize_lock: bool,
    /// The cropped size `resize_text` was filled from.
    pub resize_basis: [u32; 2],
    pub preview: Option<Preview>,
    pub preview_task: Task<Option<Preview>>,

    pub jobs: Vec<Task<Done>>,
    pub confirm: Option<Confirm>,
    pub installed: Option<HashSet<String>>,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config, open: Option<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        let theme = ThemeManager::new(ctx, &cfg.appearance);
        let max_side = cc
            .gl
            .as_ref()
            .map(|gl| {
                use eframe::glow::HasContext;
                // SAFETY: a plain query on the current context.
                unsafe { gl.get_parameter_i32(eframe::glow::MAX_TEXTURE_SIZE) as u32 }
            })
            .unwrap_or(8192)
            .clamp(2048, 16384);
        let software_gl = cc.gl.as_ref().is_some_and(|gl| {
            use eframe::glow::HasContext;
            // SAFETY: a plain query on the current context.
            let renderer =
                unsafe { gl.get_parameter_string(eframe::glow::RENDERER) }.to_lowercase();
            ["llvmpipe", "softpipe", "swiftshader", "software"]
                .iter()
                .any(|s| renderer.contains(s))
        });
        let (player, player_error) = match cc.get_proc_address.clone() {
            Some(get) => match Player::new(ctx, get, software_gl) {
                Ok(p) => (Some(p), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, Some("Video needs the OpenGL renderer".into())),
        };
        if let Some(p) = &player {
            p.set_f64("volume", cfg.volume);
            p.set_loop(cfg.loop_video);
        }
        let mut app = Self {
            theme,
            toasts: Toasts::default(),
            loader: Loader::new(ctx, max_side),
            player,
            player_error,
            files: Vec::new(),
            index: 0,
            zoom: None,
            offset: Vec2::ZERO,
            anim_start: Instant::now(),
            fullscreen: false,
            sheet: Sheet::None,
            slideshow: None,
            last_activity: Instant::now(),
            scroll_filmstrip: true,
            pending_zoom: None,
            shown_zoom: 1.0,
            controls_shown: false,
            edits: Edits::default(),
            crop: None,
            resize_text: [String::new(), String::new()],
            resize_lock: true,
            resize_basis: [0, 0],
            preview: None,
            preview_task: Task::default(),
            jobs: Vec::new(),
            confirm: None,
            installed: None,
            cfg,
        };
        if let Some(path) = open {
            app.open_path(ctx, &path);
        }
        app
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    pub fn current_kind(&self) -> Option<Kind> {
        self.current()
            .map(|p| media::kind_of(p).unwrap_or(Kind::Image))
    }

    pub fn is_video(&self) -> bool {
        self.current_kind() == Some(Kind::Video)
    }

    /// The decoded current picture, if ready.
    pub fn picture(&mut self) -> Option<Arc<Picture>> {
        match self.current_slot() {
            Some(Slot::Ready(p)) => Some(p.clone()),
            _ => None,
        }
    }

    pub fn current_slot(&mut self) -> Option<&Slot> {
        let path = self.current()?.clone();
        if media::kind_of(&path) == Some(Kind::Video) {
            return None;
        }
        let n = self.files.len();
        let neighbours: Vec<PathBuf> = [1isize, -1, 2, -2]
            .iter()
            .filter(|_| n > 1)
            .map(|d| self.files[(self.index as isize + d).rem_euclid(n as isize) as usize].clone())
            .collect();
        Some(self.loader.request(&path, &neighbours))
    }

    pub fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        if !path.exists() {
            self.toasts
                .error(format!("{} doesn't exist", path.display()));
            return;
        }
        let (files, index) = media::siblings(path);
        if files.is_empty() {
            self.toasts
                .error(format!("Nothing to show in {}", path.display()));
            return;
        }
        self.files = files;
        self.index = index;
        self.on_current_changed(ctx);
    }

    pub fn go_to(&mut self, ctx: &egui::Context, index: usize) {
        if index < self.files.len() && index != self.index {
            self.index = index;
            self.on_current_changed(ctx);
        }
    }

    pub fn step(&mut self, ctx: &egui::Context, delta: isize) {
        let n = self.files.len();
        if n > 1 {
            let i = (self.index as isize + delta).rem_euclid(n as isize) as usize;
            self.go_to(ctx, i);
        }
    }

    pub fn on_current_changed(&mut self, ctx: &egui::Context) {
        self.zoom = None;
        self.offset = Vec2::ZERO;
        self.anim_start = Instant::now();
        self.edits = Edits::default();
        self.crop = None;
        self.resize_text = [String::new(), String::new()];
        self.preview = None;
        self.scroll_filmstrip = true;
        if self.slideshow.is_some() {
            self.slideshow = Some(Instant::now());
        }
        let current = self.current().cloned();
        if let Some(player) = &mut self.player {
            match &current {
                Some(p) if media::kind_of(p) == Some(Kind::Video) => player.load(p, false),
                _ if player.file.is_some() => player.stop(),
                _ => {}
            }
        }
        let title = match &current {
            Some(p) => format!(
                "{} — Viewer",
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
            None => "Viewer".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    pub fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    pub fn toggle_slideshow(&mut self, ctx: &egui::Context) {
        if self.slideshow.take().is_none() {
            self.slideshow = Some(Instant::now());
            if !self.fullscreen {
                self.set_fullscreen(ctx, true);
            }
            self.toasts.info("Slideshow — press Esc to stop");
        }
    }

    pub fn save_config(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.toasts.error(format!("Couldn't save preferences: {e}"));
        }
    }

    pub fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Done + Send + 'static) {
        let mut task = Task::default();
        task.spawn(ctx, job);
        self.jobs.push(task);
    }

    pub fn busy(&self) -> bool {
        self.jobs.iter().any(Task::running)
    }

    // ---- actions --------------------------------------------------------

    pub fn open_dialog(&mut self, ctx: &egui::Context, folder: bool) {
        let dir = self
            .current()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        self.spawn(ctx, move || {
            let mut dialog = rfd::FileDialog::new();
            if let Some(dir) = dir {
                dialog = dialog.set_directory(dir);
            }
            Done::Opened(if folder {
                dialog.set_title("Open a folder").pick_folder()
            } else {
                dialog
                    .set_title("Open a picture or video")
                    .add_filter(
                        "Pictures and videos",
                        &[
                            "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "svg",
                            "ico", "qoi", "tga", "dng", "cr2", "nef", "arw", "mp4", "mkv", "webm",
                            "mov", "avi",
                        ],
                    )
                    .add_filter("All files", &["*"])
                    .pick_file()
            })
        });
    }

    pub fn set_wallpaper(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        let choice = self.cfg.wallpaper;
        self.toasts.info("Setting wallpaper…");
        self.spawn(ctx, move || {
            Done::Message(crate::actions::set_wallpaper(&path, choice))
        });
    }

    pub fn copy_image(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        let Some(kind) = self.current_kind().filter(|k| !k.is_video()) else {
            return;
        };
        let edits = self.edits.clone();
        self.spawn(ctx, move || {
            let result = (|| -> Result<String, String> {
                let png = if edits.is_identity()
                    && image::ImageFormat::from_path(&path).ok() == Some(image::ImageFormat::Png)
                {
                    std::fs::read(&path).map_err(|e| e.to_string())?
                } else {
                    let img = crate::decode::decode_full(&path, kind).map_err(|e| e.to_string())?;
                    let out = edits.apply(&img, true, true);
                    let mut png = Vec::new();
                    out.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                        .map_err(|e| e.to_string())?;
                    png
                };
                crate::actions::copy_image(&png)?;
                Ok("Copied to the clipboard".into())
            })();
            Done::Message(result)
        });
    }

    pub fn open_folder(&mut self) {
        if let Some(path) = self.current() {
            if let Err(e) = crate::actions::open_folder(path) {
                self.toasts.error(e);
            }
        }
    }

    pub fn request_trash(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        if self.cfg.confirm_trash {
            self.confirm = Some(Confirm::Trash(path));
        } else {
            self.trash(ctx, path);
        }
    }

    pub fn trash(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.player.as_ref().and_then(|p| p.file.as_ref()) == Some(&path) {
            if let Some(p) = &mut self.player {
                p.stop();
            }
        }
        self.spawn(ctx, move || {
            let r = crate::actions::trash(&path);
            Done::Trashed(path, r)
        });
    }

    /// Apply the edits to the full-size image and write it to `target`.
    pub fn save_to(&mut self, ctx: &egui::Context, target: PathBuf) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        let Some(kind) = self.current_kind() else {
            return;
        };
        let edits = self.edits.clone();
        self.spawn(ctx, move || {
            Done::Saved(write_edited(&path, kind, &edits, target))
        });
    }

    pub fn save_as(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        let suggested = crate::edit::edited_name(&path);
        let Some(kind) = self.current_kind() else {
            return;
        };
        let edits = self.edits.clone();
        self.spawn(ctx, move || {
            let Some(target) = rfd::FileDialog::new()
                .set_title("Save edited picture as")
                .set_directory(suggested.parent().unwrap_or(Path::new(".")))
                .set_file_name(suggested.file_name().unwrap_or_default().to_string_lossy())
                .save_file()
            else {
                return Done::Message(Ok(String::new()));
            };
            let target = if image::ImageFormat::from_path(&target).is_err() {
                target.with_extension("png")
            } else {
                target
            };
            Done::Saved(write_edited(&path, kind, &edits, target))
        });
    }

    pub fn frame_screenshot(&mut self) {
        let (Some(player), Some(path)) = (&self.player, self.current()) else {
            return;
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let dir = path.parent().unwrap_or(Path::new("."));
        let t = player.state.position;
        let target = (1..)
            .map(|n| {
                let suffix = if n == 1 {
                    String::new()
                } else {
                    format!(" ({n})")
                };
                dir.join(format!(
                    "{stem} {}{suffix}.png",
                    video::format_time(t).replace(':', "-")
                ))
            })
            .find(|p| !p.exists())
            .expect("unbounded");
        match player.screenshot(&target) {
            Ok(()) => {
                self.toasts.success(format!(
                    "Saved {}",
                    target.file_name().unwrap_or_default().to_string_lossy()
                ));
                self.refresh_files();
            }
            Err(e) => self.toasts.error(format!("Couldn't save the frame: {e}")),
        }
    }

    /// Re-read the folder (after a save added a file), keeping the current one.
    pub fn refresh_files(&mut self) {
        let Some(current) = self.current().cloned() else {
            return;
        };
        let (files, index) = media::siblings(&current);
        self.files = files;
        self.index = index;
    }

    pub fn install_mpv(&mut self) {
        let script = material_3_ui::sys::arch::install_command(&["mpv"], self.cfg.aur_helper);
        match material_3_ui::sys::term::run(&self.cfg.terminal, &script) {
            Ok(_) => self
                .toasts
                .info("Installing mpv in a terminal — restart the viewer afterwards"),
            Err(e) => self.toasts.error(e),
        }
    }

    // ---- per-frame upkeep ----------------------------------------------

    fn finish_jobs(&mut self, ctx: &egui::Context) {
        let mut finished = Vec::new();
        self.jobs.retain_mut(|t| match t.take() {
            Some(done) => {
                finished.push(done);
                false
            }
            None => t.running(),
        });
        for done in finished {
            match done {
                Done::Message(Ok(m)) if m.is_empty() => {}
                Done::Message(Ok(m)) => self.toasts.success(m),
                Done::Message(Err(e)) => self.toasts.error(e),
                Done::Opened(Some(p)) => self.open_path(ctx, &p),
                Done::Opened(None) => {}
                Done::Saved(Ok(target)) => {
                    let name = target
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.loader.invalidate(&target);
                    if Some(&target) == self.current() {
                        self.on_current_changed(ctx);
                    } else {
                        self.refresh_files();
                    }
                    self.toasts.success(format!("Saved {name}"));
                }
                Done::Saved(Err(e)) => self.toasts.error(format!("Couldn't save: {e}")),
                Done::Trashed(path, Ok(())) => {
                    self.loader.invalidate(&path);
                    if let Some(i) = self.files.iter().position(|p| *p == path) {
                        self.files.remove(i);
                        let was_current = i == self.index;
                        if i < self.index {
                            self.index -= 1;
                        }
                        self.index = self.index.min(self.files.len().saturating_sub(1));
                        if was_current {
                            self.on_current_changed(ctx);
                        }
                    }
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.toasts.success(format!("Moved {name} to the trash"));
                }
                Done::Trashed(_, Err(e)) => self
                    .toasts
                    .error(format!("Couldn't move to the trash: {e}")),
            }
        }
    }

    fn poll_player(&mut self, ctx: &egui::Context) {
        let Some(player) = &mut self.player else {
            return;
        };
        let events = player.poll();
        let mut advance = false;
        for ev in events {
            match ev {
                video::Event::Ended { error: Some(e) } => {
                    self.toasts.error(format!("Can't play this video: {e}"));
                    advance = self.slideshow.is_some();
                }
                video::Event::Ended { error: None } => {}
                video::Event::Loaded | video::Event::Resized => {}
            }
        }
        let st = &player.state;
        if self.slideshow.is_some() && player.file.is_some() && st.eof && !self.cfg.loop_video {
            advance = true;
        }
        if advance {
            self.step(ctx, 1);
        }
    }

    fn update_slideshow(&mut self, ctx: &egui::Context) {
        let Some(since) = self.slideshow else { return };
        if self.is_video() {
            return;
        }
        // Wait for the picture to load before starting its timer.
        if !matches!(self.current_slot(), Some(Slot::Ready(_) | Slot::Failed(_))) {
            self.slideshow = Some(Instant::now());
            return;
        }
        let interval = Duration::from_secs_f32(self.cfg.slideshow_seconds.max(0.5));
        let elapsed = since.elapsed();
        if elapsed >= interval {
            self.step(ctx, 1);
            self.slideshow = Some(Instant::now());
        } else {
            ctx.request_repaint_after(interval - elapsed);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(first) = dropped.first() {
            self.open_path(ctx, first);
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.confirm.is_some() {
            return;
        }
        let (pressed, mods) = ctx.input(|i| {
            let keys: Vec<Key> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => Some(*key),
                    _ => None,
                })
                .collect();
            (keys, i.modifiers)
        });
        let video = self.is_video();
        for key in pressed {
            self.last_activity = Instant::now();
            match key {
                Key::ArrowLeft | Key::ArrowRight if video && mods.shift => {
                    if let Some(p) = &self.player {
                        p.seek_by(if key == Key::ArrowLeft { -5.0 } else { 5.0 });
                    }
                }
                Key::ArrowLeft | Key::PageUp => self.step(ctx, -1),
                Key::ArrowRight | Key::PageDown => self.step(ctx, 1),
                Key::Home => self.go_to(ctx, 0),
                Key::End => self.go_to(ctx, self.files.len().saturating_sub(1)),
                Key::Space => {
                    if video {
                        if let Some(p) = &mut self.player {
                            p.toggle_pause();
                        }
                    } else {
                        self.step(ctx, 1);
                    }
                }
                Key::Comma | Key::Period if video => {
                    if let Some(p) = &self.player {
                        p.frame_step(key == Key::Period);
                    }
                }
                Key::M if video => {
                    if let Some(p) = &self.player {
                        p.set("mute", if p.state.muted { "no" } else { "yes" });
                    }
                }
                Key::Plus | Key::Equals => self.pending_zoom = Some(1.25),
                Key::Minus => self.pending_zoom = Some(0.8),
                Key::Num0 => {
                    self.zoom = Some(1.0);
                    self.offset = Vec2::ZERO;
                }
                Key::F if !mods.ctrl => {
                    self.zoom = None;
                    self.offset = Vec2::ZERO;
                }
                Key::F11 => self.set_fullscreen(ctx, !self.fullscreen),
                Key::R if !video && !mods.ctrl => {
                    if mods.shift {
                        self.edits.rotate_ccw();
                    } else {
                        self.edits.rotate_cw();
                    }
                }
                Key::I if !mods.ctrl => {
                    self.sheet = if self.sheet == Sheet::Info {
                        Sheet::None
                    } else {
                        Sheet::Info
                    };
                }
                Key::E if !video && !mods.ctrl => {
                    self.sheet = if self.sheet == Sheet::Edit {
                        Sheet::None
                    } else {
                        Sheet::Edit
                    };
                }
                Key::Delete => self.request_trash(ctx),
                Key::O if mods.command => self.open_dialog(ctx, false),
                Key::S if mods.command && !video && !self.edits.is_identity() => {
                    if mods.shift {
                        self.save_as(ctx);
                    } else {
                        self.confirm_overwrite();
                    }
                }
                Key::C if mods.command && !video => self.copy_image(ctx),
                Key::Escape => {
                    if self.crop.is_some() {
                        self.crop = None;
                    } else if self.slideshow.is_some() {
                        self.slideshow = None;
                        self.set_fullscreen(ctx, false);
                    } else if self.fullscreen {
                        self.set_fullscreen(ctx, false);
                    } else if self.sheet != Sheet::None {
                        self.sheet = Sheet::None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Save over the original after asking.
    pub fn confirm_overwrite(&mut self) {
        if let Some(path) = self.current().cloned() {
            if media::can_save(&path) {
                self.confirm = Some(Confirm::Overwrite(path));
            }
        }
    }

    /// Keep the edit preview in step with the edits.
    fn update_preview(&mut self, ctx: &egui::Context) {
        if let Some(Some(done)) = self.preview_task.take() {
            if Some(&done.key.path) == self.current() {
                self.preview = Some(done);
            }
        }
        let Some(picture) = self.picture() else {
            return;
        };
        let with_crop = self.crop.is_none();
        let mut edits = self.edits.clone();
        if !with_crop {
            edits.crop = Crop::FULL;
        }
        if edits.is_identity() {
            self.preview = None;
            return;
        }
        let key = PreviewKey {
            path: picture.path.clone(),
            edits,
            with_crop,
        };
        if self.preview.as_ref().is_some_and(|p| p.key == key) || self.preview_task.running() {
            return;
        }
        let ctx2 = ctx.clone();
        self.preview_task.spawn(ctx, move || {
            // Edit a screen-sized copy so sliders stay responsive.
            let src = crate::decode::fit((*picture.image).clone(), 2560);
            let out = key.edits.apply(&src, key.with_crop, false);
            let texture = ctx2.load_texture(
                format!("preview:{}", key.path.display()),
                crate::decode::to_color_image(&out),
                crate::loader::texture_options(),
            );
            Some(Preview { key, texture })
        });
    }
}

fn write_edited(
    path: &Path,
    kind: Kind,
    edits: &Edits,
    target: PathBuf,
) -> Result<PathBuf, String> {
    let img = crate::decode::decode_full(path, kind).map_err(|e| e.to_string())?;
    crate::edit::save(&edits.apply(&img, true, true), &target).map_err(|e| e.to_string())?;
    Ok(target)
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.theme.poll(&self.cfg.appearance);
        if let Some(e) = self.theme.take_error() {
            self.toasts.error(e);
        }
        self.fullscreen = ctx
            .input(|i| i.viewport().fullscreen)
            .unwrap_or(self.fullscreen);
        if ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
            self.last_activity = Instant::now();
        }
        self.finish_jobs(&ctx);
        self.handle_drops(&ctx);
        self.poll_player(&ctx);
        self.keyboard(&ctx);
        self.update_slideshow(&ctx);
        self.update_preview(&ctx);
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        self.draw(ui);

        let p = self.theme.palette().clone();
        let bottom = if self.fullscreen { 24.0 } else { 120.0 };
        self.toasts.show(&ctx, &p, bottom);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().surface.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(p) = &mut self.player {
            self.cfg.volume = p.state.volume;
            p.shutdown_gl(gl);
        }
        let _ = self.cfg.save();
    }
}
