//! Turning files into pixels: still and animated images, SVG, camera RAW,
//! and video thumbnails. Everything here runs off the UI thread.

use crate::media::Kind;
use anyhow::{anyhow, Context as _, Result};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;
use std::time::Duration;

/// Animations whose frames would take more memory than this are scaled down.
const MAX_ANIMATION_BYTES: u64 = 768 * 1024 * 1024;

pub struct Decoded {
    /// The first (or only) frame, oriented, and no larger than `max_side`.
    pub image: DynamicImage,
    /// Further frames of an animation, with how long each one shows. Empty
    /// for still images. Includes the first frame again when animated.
    pub frames: Vec<(RgbaImage, Duration)>,
    /// Size of the full image after EXIF orientation, before any scaling.
    pub full_size: [u32; 2],
    /// Format label for the info sheet ("PNG", "Animated WebP", …).
    pub format: String,
}

/// Decode `path` for display, at most `max_side` pixels on its long side.
pub fn decode(path: &Path, kind: Kind, max_side: u32) -> Result<Decoded> {
    match kind {
        Kind::Svg => decode_svg(path, max_side),
        Kind::Raw => {
            let image = decode_raw(path)?;
            let full_size = [image.width(), image.height()];
            Ok(Decoded {
                image: fit(image, max_side),
                frames: Vec::new(),
                full_size,
                format: format!("RAW ({})", ext_label(path)),
            })
        }
        Kind::Video => Err(anyhow!("videos are played, not decoded")),
        Kind::Image => {
            if let Some(animated) = decode_animation(path, max_side)? {
                return Ok(animated);
            }
            let (image, format) = decode_still(path)?;
            let full_size = [image.width(), image.height()];
            Ok(Decoded {
                image: fit(image, max_side),
                frames: Vec::new(),
                full_size,
                format,
            })
        }
    }
}

/// Decode at full size, for saving edits. Animated files give their first
/// frame.
pub fn decode_full(path: &Path, kind: Kind) -> Result<DynamicImage> {
    match kind {
        Kind::Image => decode_still(path).map(|(img, _)| img),
        Kind::Raw => decode_raw(path),
        Kind::Svg => decode_svg(path, 8192).map(|d| d.image),
        Kind::Video => Err(anyhow!("can't edit videos")),
    }
}

/// A still image with its EXIF orientation applied.
fn decode_still(path: &Path) -> Result<(DynamicImage, String)> {
    let reader = ImageReader::open(path)
        .with_context(|| format!("can't open {}", path.display()))?
        .with_guessed_format()?;
    let format = reader
        .format()
        .map(|f| format!("{f:?}").to_uppercase())
        .unwrap_or_else(|| ext_label(path));
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok((image, format))
}

fn ext_label(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "Image".into())
}

/// Scale down so neither side exceeds `max_side`.
pub fn fit(image: DynamicImage, max_side: u32) -> DynamicImage {
    if image.width() <= max_side && image.height() <= max_side {
        return image;
    }
    image.resize(max_side, max_side, FilterType::Triangle)
}

/// GIF, WebP and APNG animations. `None` when the file is a still image.
fn decode_animation(path: &Path, max_side: u32) -> Result<Option<Decoded>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let open =
        || -> Result<BufReader<std::fs::File>> { Ok(BufReader::new(std::fs::File::open(path)?)) };
    let mut magic = [0u8; 12];
    let n = std::fs::File::open(path)?.read(&mut magic)?;
    let magic = &magic[..n];
    let (frames, label) = if magic.starts_with(b"GIF8") {
        (
            GifDecoder::new(open()?)?.into_frames().collect_frames()?,
            "GIF",
        )
    } else if magic.len() >= 12 && &magic[0..4] == b"RIFF" && &magic[8..12] == b"WEBP" {
        let decoder = WebPDecoder::new(open()?)?;
        if !decoder.has_animation() {
            return Ok(None);
        }
        (decoder.into_frames().collect_frames()?, "WebP")
    } else if magic.starts_with(b"\x89PNG") && (ext == "png" || ext == "apng") {
        let decoder = PngDecoder::new(open()?)?;
        if !decoder.is_apng()? {
            return Ok(None);
        }
        (decoder.apng()?.into_frames().collect_frames()?, "PNG")
    } else {
        return Ok(None);
    };
    if frames.len() < 2 {
        return Ok(frames.into_iter().next().map(|f| {
            let image = DynamicImage::ImageRgba8(f.into_buffer());
            let full_size = [image.width(), image.height()];
            Decoded {
                image: fit(image, max_side),
                frames: Vec::new(),
                full_size,
                format: label.to_string(),
            }
        }));
    }
    let (w, h) = frames[0].buffer().dimensions();
    let full_size = [w, h];
    // Keep the whole animation within the memory budget and texture limit.
    let per_frame = (w as u64 * h as u64 * 4).max(1);
    let budget_scale = ((MAX_ANIMATION_BYTES / frames.len() as u64) as f64 / per_frame as f64)
        .sqrt()
        .min(1.0);
    let side_scale = (max_side as f64 / w.max(h) as f64).min(1.0);
    let scale = budget_scale.min(side_scale);
    let (tw, th) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let frames: Vec<(RgbaImage, Duration)> = frames
        .into_iter()
        .map(|f| {
            let (num, den) = f.delay().numer_denom_ms();
            let ms = num.checked_div(den).unwrap_or(100);
            // Browsers treat tiny delays as 100 ms; do the same.
            let delay = Duration::from_millis(if ms <= 10 { 100 } else { ms as u64 });
            let buf = f.into_buffer();
            let buf = if scale < 1.0 {
                image::imageops::resize(&buf, tw, th, FilterType::Triangle)
            } else {
                buf
            };
            (buf, delay)
        })
        .collect();
    Ok(Some(Decoded {
        image: DynamicImage::ImageRgba8(frames[0].0.clone()),
        frames,
        full_size,
        format: format!("Animated {label}"),
    }))
}

fn decode_svg(path: &Path, max_side: u32) -> Result<Decoded> {
    // usvg unpacks .svgz itself.
    let data = std::fs::read(path)?;
    let mut options = resvg::usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        ..Default::default()
    };
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(&data, &options)?;
    let size = tree.size();
    let (w, h) = (size.width().max(1.0), size.height().max(1.0));
    // Vector art: render crisp at a comfortable size, not at its nominal one.
    let long = w.max(h);
    let target = (long.max(2048.0)).min(max_side as f32);
    let scale = target / long;
    let (pw, ph) = (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    );
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| anyhow!("SVG is too large"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied alpha.
    let mut rgba = pixmap.take();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let image = RgbaImage::from_raw(pw, ph, rgba).ok_or_else(|| anyhow!("bad SVG buffer"))?;
    Ok(Decoded {
        image: DynamicImage::ImageRgba8(image),
        frames: Vec::new(),
        full_size: [w.round() as u32, h.round() as u32],
        format: "SVG".into(),
    })
}

#[cfg(feature = "raw")]
fn decode_raw(path: &Path) -> Result<DynamicImage> {
    match imagepipe::simple_decode_8bit(path, 0, 0) {
        Ok(img) => {
            let buf = image::RgbImage::from_raw(img.width as u32, img.height as u32, img.data)
                .ok_or_else(|| anyhow!("bad RAW buffer"))?;
            Ok(DynamicImage::ImageRgb8(buf))
        }
        Err(e) => embedded_preview(path).map_err(|_| anyhow!("can't decode RAW: {e}")),
    }
}

#[cfg(not(feature = "raw"))]
fn decode_raw(path: &Path) -> Result<DynamicImage> {
    embedded_preview(path)
}

/// The biggest JPEG preview embedded in a RAW file, oriented by the RAW's
/// own EXIF.
fn embedded_preview(path: &Path) -> Result<DynamicImage> {
    let data = std::fs::read(path)?;
    let mut best: Option<(u64, usize)> = None;
    let mut tried = 0;
    let mut i = 0;
    while i + 3 < data.len() && tried < 32 {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            tried += 1;
            if let Ok(d) = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&data[i..])) {
                let (w, h) = d.dimensions();
                let area = w as u64 * h as u64;
                if best.is_none_or(|(a, _)| area > a) {
                    best = Some((area, i));
                }
            }
        }
        i += 1;
    }
    let (_, start) = best.ok_or_else(|| {
        anyhow!("no preview in this RAW file (build with the `raw` feature to decode RAW data)")
    })?;
    let mut image = image::load_from_memory_with_format(&data[start..], image::ImageFormat::Jpeg)?;
    if let Some(o) = crate::exif::orientation(path).and_then(Orientation::from_exif) {
        image.apply_orientation(o);
    }
    Ok(image)
}

/// A small thumbnail for the filmstrip.
pub fn thumbnail(path: &Path, kind: Kind, size: u32) -> Result<RgbaImage> {
    if kind == Kind::Video {
        return video_thumbnail(path, size);
    }
    let decoded = decode(path, kind, size.max(64) * 4)?;
    Ok(decoded.image.thumbnail(size, size).to_rgba8())
}

/// A frame from a video via ffmpegthumbnailer or ffmpeg, whichever exists.
fn video_thumbnail(path: &Path, size: u32) -> Result<RgbaImage> {
    use material_3_ui::sys::cmd;
    let file = path.to_string_lossy();
    let size_s = size.to_string();
    let png = if cmd::exists("ffmpegthumbnailer") {
        std::process::Command::new("ffmpegthumbnailer")
            .args(["-i", &file, "-o", "-", "-c", "png", "-s", &size_s])
            .stderr(std::process::Stdio::null())
            .output()?
            .stdout
    } else if cmd::exists("ffmpeg") {
        let scale = format!("scale={size}:{size}:force_original_aspect_ratio=decrease");
        let run = |seek: &str| {
            std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "quiet",
                    "-ss",
                    seek,
                    "-i",
                    &file,
                    "-frames:v",
                    "1",
                    "-vf",
                    &scale,
                ])
                .args(["-f", "image2pipe", "-vcodec", "png", "-"])
                .stdin(std::process::Stdio::null())
                .output()
                .map(|o| o.stdout)
        };
        // A second in skips black lead-in frames; very short clips have none.
        let out = run("1")?;
        if out.is_empty() {
            run("0")?
        } else {
            out
        }
    } else {
        return Err(anyhow!("no ffmpegthumbnailer or ffmpeg"));
    };
    if png.is_empty() {
        return Err(anyhow!("no frame"));
    }
    Ok(image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.to_rgba8())
}

pub fn to_color_image(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Frame, Rgba};

    #[test]
    fn still_png_and_downscale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        RgbaImage::from_pixel(400, 200, Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let d = decode(&path, Kind::Image, 100).unwrap();
        assert_eq!(d.full_size, [400, 200]);
        assert_eq!((d.image.width(), d.image.height()), (100, 50));
        assert!(d.frames.is_empty());
        assert_eq!(d.format, "PNG");
    }

    #[test]
    fn animated_gif_has_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anim.gif");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(file);
            for c in [0u8, 128, 255] {
                let buf = RgbaImage::from_pixel(8, 8, Rgba([c, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(
                    buf,
                    0,
                    0,
                    Delay::from_numer_denom_ms(50, 1),
                ))
                .unwrap();
            }
        }
        let d = decode(&path, Kind::Image, 4096).unwrap();
        assert_eq!(d.frames.len(), 3);
        assert_eq!(d.frames[0].1, Duration::from_millis(50));
        assert_eq!(d.format, "Animated GIF");
    }

    #[test]
    fn svg_renders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.svg");
        std::fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="red"/></svg>"#,
        )
        .unwrap();
        let d = decode(&path, Kind::Svg, 1024).unwrap();
        assert_eq!(d.full_size, [20, 10]);
        assert_eq!((d.image.width(), d.image.height()), (1024, 512));
        assert_eq!(d.image.to_rgba8().get_pixel(5, 5).0, [255, 0, 0, 255]);
    }
}
