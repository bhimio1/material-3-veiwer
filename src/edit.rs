//! Non-destructive edits. They're kept as a recipe and applied to a preview
//! while editing; the file is only touched by Save or Save as, which apply
//! the recipe to the full-size image.
//!
//! Order: rotate/flip → crop → resize → brightness/contrast/saturation.

use anyhow::{anyhow, Result};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use std::path::Path;

/// A crop, as fractions (0–1) of the rotated and flipped image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    pub const FULL: Crop = Crop {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    pub fn is_full(&self) -> bool {
        self.x <= 0.0005 && self.y <= 0.0005 && self.w >= 0.999 && self.h >= 0.999
    }

    /// Pixel rectangle inside a `w`×`h` image (at least 1×1).
    pub fn pixels(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let x = ((self.x * w as f32).round() as u32).min(w.saturating_sub(1));
        let y = ((self.y * h as f32).round() as u32).min(h.saturating_sub(1));
        let cw = ((self.w * w as f32).round() as u32).clamp(1, w - x);
        let ch = ((self.h * h as f32).round() as u32).clamp(1, h - y);
        (x, y, cw, ch)
    }

    /// Keep the crop inside the image and at least a sliver big.
    pub fn clamped(mut self) -> Crop {
        self.w = self.w.clamp(0.01, 1.0);
        self.h = self.h.clamp(0.01, 1.0);
        self.x = self.x.clamp(0.0, 1.0 - self.w);
        self.y = self.y.clamp(0.0, 1.0 - self.h);
        self
    }
}

/// Aspect ratio presets for cropping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aspect {
    Free,
    Original,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    NineSixteen,
}

impl Aspect {
    pub const ALL: [Aspect; 7] = [
        Aspect::Free,
        Aspect::Original,
        Aspect::Square,
        Aspect::FourThree,
        Aspect::ThreeTwo,
        Aspect::SixteenNine,
        Aspect::NineSixteen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Aspect::Free => "Free",
            Aspect::Original => "Original",
            Aspect::Square => "1:1",
            Aspect::FourThree => "4:3",
            Aspect::ThreeTwo => "3:2",
            Aspect::SixteenNine => "16:9",
            Aspect::NineSixteen => "9:16",
        }
    }

    /// Width / height in pixels, or `None` for free.
    pub fn ratio(self, image_w: u32, image_h: u32) -> Option<f32> {
        Some(match self {
            Aspect::Free => return None,
            Aspect::Original => image_w as f32 / image_h.max(1) as f32,
            Aspect::Square => 1.0,
            Aspect::FourThree => 4.0 / 3.0,
            Aspect::ThreeTwo => 3.0 / 2.0,
            Aspect::SixteenNine => 16.0 / 9.0,
            Aspect::NineSixteen => 9.0 / 16.0,
        })
    }

    /// The biggest crop with this aspect, centred on `around`.
    pub fn fit(self, around: Crop, image_w: u32, image_h: u32) -> Crop {
        let Some(ratio) = self.ratio(image_w, image_h) else {
            return around;
        };
        // In fraction space a pixel ratio r becomes r * h / w.
        let frac_ratio = ratio * image_h as f32 / image_w.max(1) as f32;
        let (cx, cy) = (around.x + around.w / 2.0, around.y + around.h / 2.0);
        let (mut w, mut h) = (around.w, around.w / frac_ratio);
        if h > around.h {
            h = around.h;
            w = h * frac_ratio;
        }
        // Grow to the largest that fits the image when starting from full.
        if around.is_full() {
            w = 1.0f32.min(frac_ratio);
            h = w / frac_ratio;
        }
        Crop {
            x: cx - w / 2.0,
            y: cy - h / 2.0,
            w,
            h,
        }
        .clamped()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edits {
    /// Quarter turns clockwise, 0–3.
    pub quarter_turns: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    pub crop: Crop,
    /// Output width and height; `None` keeps the size.
    pub resize: Option<[u32; 2]>,
    /// −1 … 1
    pub brightness: f32,
    /// −1 … 1
    pub contrast: f32,
    /// −1 … 1
    pub saturation: f32,
}

impl Default for Edits {
    fn default() -> Self {
        Self {
            quarter_turns: 0,
            flip_h: false,
            flip_v: false,
            crop: Crop::FULL,
            resize: None,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
        }
    }
}

impl Edits {
    pub fn is_identity(&self) -> bool {
        *self == Edits::default()
    }

    pub fn has_adjustments(&self) -> bool {
        self.brightness != 0.0 || self.contrast != 0.0 || self.saturation != 0.0
    }

    pub fn rotate_cw(&mut self) {
        self.quarter_turns = (self.quarter_turns + 1) % 4;
        // The crop is relative to the rotated image, so it turns too.
        let c = self.crop;
        self.crop = Crop {
            x: 1.0 - c.y - c.h,
            y: c.x,
            w: c.h,
            h: c.w,
        };
        self.resize = self.resize.map(|[w, h]| [h, w]);
    }

    pub fn rotate_ccw(&mut self) {
        for _ in 0..3 {
            self.rotate_cw();
        }
    }

    pub fn flip_horizontal(&mut self) {
        self.flip_h = !self.flip_h;
        self.crop.x = 1.0 - self.crop.x - self.crop.w;
    }

    pub fn flip_vertical(&mut self) {
        self.flip_v = !self.flip_v;
        self.crop.y = 1.0 - self.crop.y - self.crop.h;
    }

    /// Size after rotation, for an image of `w`×`h`.
    pub fn oriented_size(&self, w: u32, h: u32) -> [u32; 2] {
        if self.quarter_turns % 2 == 1 {
            [h, w]
        } else {
            [w, h]
        }
    }

    /// Size after rotation and crop.
    pub fn cropped_size(&self, w: u32, h: u32) -> [u32; 2] {
        let [ow, oh] = self.oriented_size(w, h);
        let (_, _, cw, ch) = self.crop.pixels(ow, oh);
        [cw, ch]
    }

    /// Final size of the saved image.
    pub fn output_size(&self, w: u32, h: u32) -> [u32; 2] {
        self.resize.unwrap_or_else(|| self.cropped_size(w, h))
    }

    /// Rotation and flips only.
    pub fn orient(&self, image: &DynamicImage) -> DynamicImage {
        let mut img = match self.quarter_turns % 4 {
            1 => image.rotate90(),
            2 => image.rotate180(),
            3 => image.rotate270(),
            _ => image.clone(),
        };
        if self.flip_h {
            img = img.fliph();
        }
        if self.flip_v {
            img = img.flipv();
        }
        img
    }

    /// Apply the whole recipe. `with_crop: false` leaves the crop out (for
    /// showing the crop overlay); `resize: false` skips resizing (previews
    /// are made from a downscaled image, so the target size means nothing).
    pub fn apply(&self, image: &DynamicImage, with_crop: bool, resize: bool) -> RgbaImage {
        let mut img = self.orient(image);
        if with_crop && !self.crop.is_full() {
            let (x, y, w, h) = self.crop.pixels(img.width(), img.height());
            img = img.crop_imm(x, y, w, h);
        }
        if resize {
            if let Some([w, h]) = self.resize {
                if [w, h] != [img.width(), img.height()] && w > 0 && h > 0 {
                    img = img.resize_exact(w, h, FilterType::Lanczos3);
                }
            }
        }
        let mut rgba = img.to_rgba8();
        if self.has_adjustments() {
            adjust(&mut rgba, self.brightness, self.contrast, self.saturation);
        }
        rgba
    }
}

/// Brightness, contrast and saturation, in place.
pub fn adjust(img: &mut RgbaImage, brightness: f32, contrast: f32, saturation: f32) {
    // Contrast pivots around mid-grey; positive values steepen up to 3×.
    let factor = if contrast >= 0.0 {
        1.0 + contrast * 2.0
    } else {
        1.0 + contrast
    };
    let lut: Vec<u8> = (0..256)
        .map(|v| {
            let x = v as f32 / 255.0;
            let y = (x - 0.5) * factor + 0.5 + brightness * 0.5;
            (y.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    let sat = 1.0 + saturation;
    for px in img.pixels_mut() {
        let [r, g, b, _] = px.0;
        let (mut r, mut g, mut b) = (r as f32, g as f32, b as f32);
        if saturation != 0.0 {
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            r = (l + (r - l) * sat).clamp(0.0, 255.0);
            g = (l + (g - l) * sat).clamp(0.0, 255.0);
            b = (l + (b - l) * sat).clamp(0.0, 255.0);
        }
        px.0[0] = lut[r.round() as usize];
        px.0[1] = lut[g.round() as usize];
        px.0[2] = lut[b.round() as usize];
    }
}

/// Write `image` to `path` in the format its extension names, through a
/// temporary file so a failed save never leaves a half-written picture.
pub fn save(image: &RgbaImage, path: &Path) -> Result<()> {
    let format = image::ImageFormat::from_path(path)
        .map_err(|_| anyhow!("unknown image format for {}", path.display()))?;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| anyhow!("no file name"))?;
    let tmp = dir.join(format!(".{}.m3-viewer-tmp", name.to_string_lossy()));
    let result = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        match format {
            image::ImageFormat::Jpeg => {
                let rgb = DynamicImage::ImageRgba8(image.clone()).to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
                    .encode_image(&rgb)?;
            }
            image::ImageFormat::WebP => {
                image::codecs::webp::WebPEncoder::new_lossless(&mut out).encode(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
            }
            _ => image.write_to(&mut out, format)?,
        }
        std::io::Write::flush(&mut out)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            std::fs::rename(&tmp, path)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// "photo.jpg" → "photo (edited).jpg", "photo (edited 2).jpg", … whichever
/// doesn't exist yet.
pub fn edited_name(path: &Path) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .filter(|e| crate::media::can_save(Path::new(&format!("x.{e}"))))
        .unwrap_or_else(|| "png".into());
    let dir = path.parent().unwrap_or(Path::new("."));
    (1..)
        .map(|n| {
            let suffix = if n == 1 {
                " (edited)".to_string()
            } else {
                format!(" (edited {n})")
            };
            dir.join(format!("{stem}{suffix}.{ext}"))
        })
        .find(|p| !p.exists())
        .expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn marked(w: u32, h: u32) -> DynamicImage {
        // Red top-left pixel on blue.
        let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 255, 255]));
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn identity_is_a_no_op() {
        let img = marked(4, 3);
        let e = Edits::default();
        assert!(e.is_identity());
        assert_eq!(e.apply(&img, true, true), img.to_rgba8());
    }

    #[test]
    fn rotation_and_flip() {
        let img = marked(4, 2);
        let mut e = Edits::default();
        e.rotate_cw();
        let out = e.apply(&img, true, true);
        assert_eq!(out.dimensions(), (2, 4));
        assert_eq!(out.get_pixel(1, 0).0, [255, 0, 0, 255]);
        e.flip_horizontal();
        let out = e.apply(&img, true, true);
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
        e.rotate_ccw();
        assert_eq!(e.quarter_turns, 0);
    }

    #[test]
    fn crop_follows_rotation() {
        // Crop the left half, then rotate: it becomes the top half.
        let mut e = Edits {
            crop: Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            },
            ..Default::default()
        };
        e.rotate_cw();
        assert_eq!(
            e.crop,
            Crop {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 0.5
            }
        );
        let img = marked(8, 4);
        let out = e.apply(&img, true, true);
        assert_eq!(out.dimensions(), (4, 4));
        // The marked pixel was in the kept half and lands in the top right.
        assert_eq!(out.get_pixel(3, 0).0, [255, 0, 0, 255]);
        assert_eq!(e.cropped_size(8, 4), [4, 4]);
    }

    #[test]
    fn crop_pixels_and_resize() {
        let img = marked(100, 50);
        let e = Edits {
            crop: Crop {
                x: 0.1,
                y: 0.2,
                w: 0.5,
                h: 0.5,
            },
            resize: Some([10, 5]),
            ..Default::default()
        };
        assert_eq!(e.crop.pixels(100, 50), (10, 10, 50, 25));
        assert_eq!(e.apply(&img, true, true).dimensions(), (10, 5));
        assert_eq!(e.apply(&img, false, false).dimensions(), (100, 50));
        assert_eq!(e.output_size(100, 50), [10, 5]);
    }

    #[test]
    fn aspect_presets() {
        let c = Aspect::Square.fit(Crop::FULL, 200, 100);
        let (_, _, w, h) = c.pixels(200, 100);
        assert_eq!((w, h), (100, 100));
        let c = Aspect::SixteenNine.fit(Crop::FULL, 1000, 1000);
        let (_, _, w, h) = c.pixels(1000, 1000);
        assert_eq!(w, 1000);
        assert!((h as i32 - 563).abs() <= 1);
        assert_eq!(Aspect::Free.fit(Crop::FULL, 10, 10), Crop::FULL);
    }

    #[test]
    fn adjustments() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([100, 150, 200, 255]));
        adjust(&mut img, 0.0, 0.0, -1.0);
        let [r, g, b, a] = img.get_pixel(0, 0).0;
        assert!(r == g && g == b, "desaturated to grey");
        assert_eq!(a, 255);
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([100, 100, 100, 255]));
        adjust(&mut img, 0.5, 0.0, 0.0);
        assert!(img.get_pixel(0, 0).0[0] > 100);
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([200, 200, 200, 255]));
        adjust(&mut img, 0.0, 0.5, 0.0);
        assert!(
            img.get_pixel(0, 0).0[0] > 200,
            "contrast pushes light colors lighter"
        );
    }

    #[test]
    fn save_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pic.jpg");
        let img = marked(20, 10).to_rgba8();
        save(&img, &src).unwrap();
        let back = image::open(&src).unwrap();
        assert_eq!((back.width(), back.height()), (20, 10));
        assert_eq!(edited_name(&src), dir.path().join("pic (edited).jpg"));
        save(&img, &edited_name(&src)).unwrap();
        assert_eq!(edited_name(&src), dir.path().join("pic (edited 2).jpg"));
        assert_eq!(
            edited_name(&dir.path().join("x.gif")),
            dir.path().join("x (edited).png")
        );
        let webp = dir.path().join("a.webp");
        save(&img, &webp).unwrap();
        assert_eq!(image::open(&webp).unwrap().to_rgba8(), img);
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("tmp")));
    }
}
