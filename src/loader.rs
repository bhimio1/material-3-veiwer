//! Background decoding with a small cache. The file being shown is decoded
//! first, then its neighbours, so stepping through a folder is instant.
//! Textures are uploaded from the worker threads (egui's context is
//! thread-safe), so the UI thread never stalls on a big picture.

use crate::decode::{self, to_color_image};
use crate::media::{self, Kind};
use egui::{TextureHandle, TextureOptions};
use image::DynamicImage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many decoded images to keep (current, plus neighbours either side).
const CACHE: usize = 7;
const THUMB: u32 = 160;

pub struct Picture {
    pub path: PathBuf,
    /// The decoded image (scaled to fit a texture), for editing.
    pub image: Arc<DynamicImage>,
    pub texture: TextureHandle,
    /// Animation frames; empty for still images.
    pub frames: Vec<(TextureHandle, Duration)>,
    pub full_size: [u32; 2],
    pub format: String,
    pub file_size: u64,
    pub exif: Vec<(String, String)>,
}

impl Picture {
    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// The texture to show `elapsed` into the animation.
    pub fn frame_at(&self, elapsed: Duration) -> (&TextureHandle, Duration) {
        if !self.animated() {
            return (&self.texture, Duration::MAX);
        }
        let total: Duration = self.frames.iter().map(|f| f.1).sum();
        let mut t = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos().max(1)) as u64);
        for (tex, delay) in &self.frames {
            if t < *delay {
                return (tex, *delay - t);
            }
            t -= *delay;
        }
        (&self.frames[0].0, self.frames[0].1)
    }
}

pub enum Slot {
    Loading,
    Ready(Arc<Picture>),
    Failed(String),
}

pub fn texture_options() -> TextureOptions {
    TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear))
}

type Channel<T> = (flume::Sender<T>, flume::Receiver<T>);

struct Shared {
    /// Paths to decode, most urgent first.
    queue: Mutex<VecDeque<PathBuf>>,
    /// Paths to thumbnail, newest request first.
    thumbs: Mutex<Vec<PathBuf>>,
    wake: Channel<()>,
    wake_thumbs: Channel<()>,
    done: Channel<(PathBuf, Slot)>,
    thumbs_done: Channel<(PathBuf, Option<TextureHandle>)>,
}

pub struct Loader {
    shared: Arc<Shared>,
    slots: HashMap<PathBuf, Slot>,
    /// Most recently wanted last.
    order: VecDeque<PathBuf>,
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    thumbs_pending: HashSet<PathBuf>,
}

impl Loader {
    /// `max_side` is the GPU's texture size limit; bigger images are scaled
    /// down to it.
    pub fn new(ctx: &egui::Context, max_side: u32) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            thumbs: Mutex::new(Vec::new()),
            wake: flume::unbounded(),
            wake_thumbs: flume::unbounded(),
            done: flume::unbounded(),
            thumbs_done: flume::unbounded(),
        });
        for _ in 0..2 {
            let shared = shared.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || decode_worker(&shared, &ctx, max_side));
        }
        {
            let shared = shared.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || thumb_worker(&shared, &ctx));
        }
        Self {
            shared,
            slots: HashMap::new(),
            order: VecDeque::new(),
            thumbs: HashMap::new(),
            thumbs_pending: HashSet::new(),
        }
    }

    fn collect(&mut self) {
        while let Ok((path, slot)) = self.shared.done.1.try_recv() {
            // Dropped from the cache while decoding: don't resurrect it.
            if self.slots.contains_key(&path) {
                self.slots.insert(path, slot);
            }
        }
        while let Ok((path, tex)) = self.shared.thumbs_done.1.try_recv() {
            self.thumbs_pending.remove(&path);
            self.thumbs.insert(path, tex);
        }
    }

    /// Ask for `current` now and `neighbours` next. Returns the current slot.
    pub fn request(&mut self, current: &Path, neighbours: &[PathBuf]) -> &Slot {
        self.collect();
        let wanted: Vec<&Path> = std::iter::once(current)
            .chain(neighbours.iter().map(PathBuf::as_path))
            .filter(|p| !media::kind_of(p).is_some_and(Kind::is_video))
            .collect();
        {
            let mut queue = self.shared.queue.lock().unwrap();
            queue.retain(|p| wanted.contains(&p.as_path()));
            for p in wanted.iter().rev() {
                if !self.slots.contains_key(*p) {
                    self.slots.insert(p.to_path_buf(), Slot::Loading);
                    queue.push_front(p.to_path_buf());
                }
                self.order.retain(|o| o != p);
                self.order.push_back(p.to_path_buf());
            }
            // Current first.
            if let Some(i) = queue.iter().position(|p| p == current) {
                let p = queue.remove(i).unwrap();
                queue.push_front(p);
            }
        }
        let _ = self.shared.wake.0.send(());
        while self.order.len() > CACHE {
            if let Some(old) = self.order.pop_front() {
                self.slots.remove(&old);
            }
        }
        self.slots
            .entry(current.to_path_buf())
            .or_insert(Slot::Loading)
    }

    /// Forget a file (after it was edited, renamed or trashed).
    pub fn invalidate(&mut self, path: &Path) {
        self.slots.remove(path);
        self.order.retain(|p| p != path);
        self.thumbs.remove(path);
        self.thumbs_pending.remove(path);
    }

    /// The filmstrip thumbnail for `path`, requesting it when missing.
    /// `Some(None)` means it can't be made.
    pub fn thumbnail(&mut self, path: &Path) -> Option<Option<TextureHandle>> {
        self.collect();
        if let Some(t) = self.thumbs.get(path) {
            return Some(t.clone());
        }
        if self.thumbs_pending.insert(path.to_path_buf()) {
            let mut q = self.shared.thumbs.lock().unwrap();
            q.retain(|p| p != path);
            q.push(path.to_path_buf());
            let _ = self.shared.wake_thumbs.0.send(());
        }
        None
    }

    /// Thumbnails requested but not visible any more are dropped from the
    /// queue; call once a frame after drawing the filmstrip.
    pub fn keep_thumbnails(&mut self, visible: &HashSet<PathBuf>) {
        let mut q = self.shared.thumbs.lock().unwrap();
        q.retain(|p| {
            let keep = visible.contains(p);
            if !keep {
                self.thumbs_pending.remove(p);
            }
            keep
        });
    }
}

fn decode_worker(shared: &Shared, ctx: &egui::Context, max_side: u32) {
    loop {
        let next = shared.queue.lock().unwrap().pop_front();
        let Some(path) = next else {
            if shared.wake.1.recv().is_err() {
                return;
            }
            continue;
        };
        let slot = load(ctx, &path, max_side);
        if shared.done.0.send((path, slot)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn load(ctx: &egui::Context, path: &Path, max_side: u32) -> Slot {
    // Files with an unknown extension are tried as images anyway.
    let kind = media::kind_of(path).unwrap_or(Kind::Image);
    let meta = std::fs::metadata(path);
    let decoded = match decode::decode(path, kind, max_side) {
        Ok(d) => d,
        Err(e) => return Slot::Failed(e.to_string()),
    };
    let name = path.to_string_lossy();
    let options = texture_options();
    let frames: Vec<(TextureHandle, Duration)> = decoded
        .frames
        .iter()
        .enumerate()
        .map(|(i, (img, delay))| {
            (
                ctx.load_texture(format!("{name}#{i}"), to_color_image(img), options),
                *delay,
            )
        })
        .collect();
    let texture = match frames.first() {
        Some((t, _)) => t.clone(),
        None => ctx.load_texture(
            name.to_string(),
            to_color_image(&decoded.image.to_rgba8()),
            options,
        ),
    };
    Slot::Ready(Arc::new(Picture {
        path: path.to_path_buf(),
        image: Arc::new(decoded.image),
        texture,
        frames,
        full_size: decoded.full_size,
        format: decoded.format,
        file_size: meta.map(|m| m.len()).unwrap_or(0),
        exif: crate::exif::details(path),
    }))
}

fn thumb_worker(shared: &Shared, ctx: &egui::Context) {
    loop {
        let next = shared.thumbs.lock().unwrap().pop();
        let Some(path) = next else {
            if shared.wake_thumbs.1.recv().is_err() {
                return;
            }
            continue;
        };
        let tex = media::kind_of(&path)
            .and_then(|kind| decode::thumbnail(&path, kind, THUMB).ok())
            .map(|img| {
                ctx.load_texture(
                    format!("thumb:{}", path.display()),
                    to_color_image(&img),
                    TextureOptions::LINEAR,
                )
            });
        if shared.thumbs_done.0.send((path, tex)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}
