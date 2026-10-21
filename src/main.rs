//! Material 3 Viewer: pictures and videos in the Material 3 apps' style.

mod actions;
mod app;
mod config;
mod decode;
mod edit;
mod exif;
mod loader;
mod media;
mod ui;
mod video;

use std::path::PathBuf;

const USAGE: &str = "\
Material 3 Viewer — pictures and videos

Usage: m3-viewer [FILE | FOLDER]

Opens FILE among the other pictures and videos in its folder, or the first
one in FOLDER. With no argument, shows an Open button (or drop a file in).

Keys: ←/→ previous/next, Space play/pause (next picture), +/−/0 zoom,
F fit, F11 fullscreen, R rotate, E edit, I details, Delete trash, Esc back.

Options:
  -h, --help       show this help
  -V, --version    show the version
";

fn main() -> eframe::Result {
    let mut open = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                print!("{USAGE}");
                return Ok(());
            }
            Some("-V" | "--version") => {
                println!("m3-viewer {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some(s)
                if s.starts_with('-')
                    && s != "-"
                    && open.is_none()
                    && !std::path::Path::new(s).exists() =>
            {
                eprintln!("m3-viewer: unknown option {s}\n\n{USAGE}");
                std::process::exit(2);
            }
            _ if open.is_none() && !arg.is_empty() => open = Some(file_arg(arg)),
            _ => {}
        }
    }
    video::init_locale();
    let cfg = config::Config::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Viewer")
            .with_app_id("material_3_viewer")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([480.0, 360.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "material_3_viewer",
        options,
        Box::new(move |cc| Ok(Box::new(app::ViewerApp::new(cc, cfg, open)))),
    )
}

/// A path or a file:// URL (some apps pass those).
fn file_arg(arg: std::ffi::OsString) -> PathBuf {
    if let Some(s) = arg.to_str() {
        if let Some(rest) = s.strip_prefix("file://") {
            return PathBuf::from(percent_decode(rest));
        }
    }
    PathBuf::from(arg)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls() {
        assert_eq!(
            file_arg("file:///home/a/My%20Pic.png".into()),
            PathBuf::from("/home/a/My Pic.png")
        );
        assert_eq!(file_arg("/tmp/x.png".into()), PathBuf::from("/tmp/x.png"));
        assert_eq!(percent_decode("100%"), "100%");
    }
}
