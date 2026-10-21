//! EXIF metadata for the info sheet.

use exif::{In, Reader, Tag, Value};
use std::io::BufReader;
use std::path::Path;

fn read(path: &Path) -> Option<exif::Exif> {
    let file = std::fs::File::open(path).ok()?;
    Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()
}

/// The EXIF orientation value (1–8), if the file has one.
pub fn orientation(path: &Path) -> Option<u8> {
    let exif = read(path)?;
    let field = exif.get_field(Tag::Orientation, In::PRIMARY)?;
    field.value.get_uint(0).and_then(|v| u8::try_from(v).ok())
}

/// Human-readable camera details, in a fixed order.
pub fn details(path: &Path) -> Vec<(String, String)> {
    let Some(exif) = read(path) else {
        return Vec::new();
    };
    let get = |tag: Tag| -> Option<String> {
        let f = exif.get_field(tag, In::PRIMARY)?;
        let s = f.display_value().with_unit(&exif).to_string();
        let s = s.trim().trim_matches('"').trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let mut out = Vec::new();
    let camera = match (get(Tag::Make), get(Tag::Model)) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (a, b) => a.or(b),
    };
    let mut push = |label: &str, value: Option<String>| {
        if let Some(v) = value {
            out.push((label.to_string(), v));
        }
    };
    push("Camera", camera);
    push("Lens", get(Tag::LensModel));
    push(
        "Taken",
        get(Tag::DateTimeOriginal).or_else(|| get(Tag::DateTime)),
    );
    push("Exposure", get(Tag::ExposureTime));
    push("Aperture", get(Tag::FNumber));
    push("ISO", get(Tag::PhotographicSensitivity));
    push("Focal length", get(Tag::FocalLength));
    push("Flash", get(Tag::Flash));
    push("Software", get(Tag::Software));
    push("Location", gps(&exif));
    out
}

fn gps(exif: &exif::Exif) -> Option<String> {
    let coord = |tag: Tag, reference: Tag| -> Option<f64> {
        let f = exif.get_field(tag, In::PRIMARY)?;
        let Value::Rational(ref v) = f.value else {
            return None;
        };
        if v.len() < 3 {
            return None;
        }
        let deg = v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0;
        let r = exif
            .get_field(reference, In::PRIMARY)
            .map(|f| f.display_value().to_string())
            .unwrap_or_default();
        Some(if r.contains('S') || r.contains('W') {
            -deg
        } else {
            deg
        })
    };
    let lat = coord(Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let lon = coord(Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    Some(format!("{lat:.5}, {lon:.5}"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A JPEG whose EXIF says "rotate 90° clockwise" (orientation 6), from a
    /// `w`×`h` image whose top-left pixel is red and the rest blue.
    pub fn rotated_jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([0, 0, 255]));
        for y in 0..4 {
            for x in 0..4 {
                img.put_pixel(x, y, image::Rgb([255, 0, 0]));
            }
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&img)
            .unwrap();
        // Big-endian TIFF with one IFD entry: Orientation (0x0112) = 6.
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn reads_orientation_and_decode_applies_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rot.jpg");
        std::fs::write(&path, rotated_jpeg(64, 32)).unwrap();
        assert_eq!(orientation(&path), Some(6));
        let d = crate::decode::decode(&path, crate::media::Kind::Image, 4096).unwrap();
        // 64×32 rotated a quarter turn is 32×64, and the red corner moves to
        // the top right.
        assert_eq!(d.full_size, [32, 64]);
        let rgb = d.image.to_rgb8();
        let px = rgb.get_pixel(30, 1).0;
        assert!(px[0] > 200 && px[2] < 60, "{px:?}");
        let px = rgb.get_pixel(1, 1).0;
        assert!(px[2] > 200, "{px:?}");
    }

    #[test]
    fn no_exif_no_details() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.png");
        image::RgbImage::new(2, 2).save(&path).unwrap();
        assert!(details(&path).is_empty());
        assert_eq!(orientation(&path), None);
    }
}
