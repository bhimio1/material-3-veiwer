//! What the viewer can open, and the folder it was opened in.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Svg,
    Raw,
    Video,
}

impl Kind {
    pub fn is_video(self) -> bool {
        self == Kind::Video
    }
}

const IMAGE_EXTS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "ico", "tif", "tiff", "tga",
    "pnm", "pbm", "pgm", "ppm", "pam", "qoi", "hdr",
];
const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "orf", "rw2", "raf", "pef",
    "srw", "erf", "kdc", "mrw", "3fr", "iiq", "mos",
];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "ogv", "mpg", "mpeg", "ts", "m2ts",
    "mts", "3gp", "vob",
];

/// The kind of file `path` is, judged by its extension.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let ext = ext.as_str();
    if IMAGE_EXTS.contains(&ext) {
        Some(Kind::Image)
    } else if ext == "svg" || ext == "svgz" {
        Some(Kind::Svg)
    } else if RAW_EXTS.contains(&ext) {
        // Without the `raw` feature, many RAW files still carry a full-size
        // JPEG preview; the decoder falls back to that.
        Some(Kind::Raw)
    } else if VIDEO_EXTS.contains(&ext) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// Image formats Save can write, by extension.
pub fn can_save(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "tga" | "qoi" | "ico"
            )
        })
        .unwrap_or(false)
}

/// "Natural" comparison so `img2` sorts before `img10` (same rules as the
/// file manager).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                if ca != cb {
                    return ca.cmp(&cb);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn sort_key(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Every viewable file in `dir`, natural-sorted by name. Hidden files are
/// left out.
pub fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
        })
        .filter(|p| p.is_file() && kind_of(p).is_some())
        .collect();
    files.sort_by(|a, b| natural_cmp(&sort_key(a), &sort_key(b)));
    files
}

/// The files to step through when opening `target`, and which one to show
/// first. A folder opens on its first file; a file opens among its siblings
/// (it's included even when its extension isn't one we list, so the user
/// gets an error for it rather than a different picture).
pub fn siblings(target: &Path) -> (Vec<PathBuf>, usize) {
    if target.is_dir() {
        return (list_dir(target), 0);
    }
    let target = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let Some(dir) = target.parent() else {
        return (vec![target], 0);
    };
    let mut files = list_dir(dir);
    match files.iter().position(|p| *p == target) {
        Some(i) => (files, i),
        None => {
            let key = sort_key(&target);
            let i = files
                .iter()
                .position(|p| natural_cmp(&sort_key(p), &key) == Ordering::Greater)
                .unwrap_or(files.len());
            files.insert(i, target);
            (files, i)
        }
    }
}

/// Human-readable byte count.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.png", "img2.png", "img1.png", "img02b.png", "a.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["a.png", "img1.png", "img2.png", "img02b.png", "img10.png"]
        );
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind_of(Path::new("a.JPG")), Some(Kind::Image));
        assert_eq!(kind_of(Path::new("a.svg")), Some(Kind::Svg));
        assert_eq!(kind_of(Path::new("a.mkv")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new("a.NEF")), Some(Kind::Raw));
        assert_eq!(kind_of(Path::new("a.txt")), None);
        assert_eq!(kind_of(Path::new("noext")), None);
    }

    #[test]
    fn siblings_are_sorted_and_filtered() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "b10.png",
            "b2.jpg",
            "notes.txt",
            ".hidden.png",
            "clip.webm",
            "A1.gif",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.png")).unwrap();
        let (files, i) = siblings(&dir.path().join("b10.png"));
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["A1.gif", "b2.jpg", "b10.png", "clip.webm"]);
        assert_eq!(i, 2);

        let (files, i) = siblings(dir.path());
        assert_eq!(files.len(), 4);
        assert_eq!(i, 0);
    }

    #[test]
    fn unknown_file_is_kept_in_place() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "c.png", "b.dat"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let (files, i) = siblings(&dir.path().join("b.dat"));
        assert_eq!(files.len(), 3);
        assert_eq!(i, 1);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1_500_000), "1.5 MB");
    }
}
