//! Things done to the current file outside the viewer: wallpaper,
//! clipboard, file manager, trash. These block, so call them off the UI
//! thread.

use crate::config::Wallpaper;
use material_3_ui::sys::{clipboard, cmd, paths};
use std::path::{Path, PathBuf};

/// end4-pC's (or illogical-impulse's) wallpaper script, if installed.
pub fn switchwall_script() -> Option<PathBuf> {
    let root = paths::config_home().join("quickshell");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path().join("scripts/colors/switchwall.sh"))
        .filter(|p| p.is_file())
        .collect();
    // Prefer the fork, then upstream illogical-impulse, then anything else.
    found.sort_by_key(|p| {
        let shell = p
            .ancestors()
            .nth(3)
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match shell.as_str() {
            "end4-pc" => 0,
            "ii" => 1,
            _ => 2,
        }
    });
    found.into_iter().next()
}

/// Which backend `choice` resolves to on this system.
pub fn wallpaper_backend(choice: Wallpaper) -> Option<Wallpaper> {
    let end4 = || switchwall_script().is_some();
    match choice {
        Wallpaper::Auto => {
            if end4() {
                Some(Wallpaper::End4)
            } else if cmd::exists("swww") {
                Some(Wallpaper::Swww)
            } else if cmd::exists("hyprctl") {
                Some(Wallpaper::Hyprpaper)
            } else {
                None
            }
        }
        other => Some(other),
    }
}

/// Make `path` the desktop wallpaper. end4-pC's script also regenerates the
/// color scheme from it, so every Material 3 app follows.
pub fn set_wallpaper(path: &Path, choice: Wallpaper) -> Result<String, String> {
    let file = path.to_string_lossy();
    match wallpaper_backend(choice) {
        Some(Wallpaper::End4) => {
            let script = switchwall_script().ok_or("end4-pC's switchwall.sh isn't installed")?;
            cmd::run("bash", &[&script.to_string_lossy(), "--image", &file])?;
            Ok("Wallpaper set; colors are updating".into())
        }
        Some(Wallpaper::Swww) => {
            cmd::run("swww", &["img", &file, "--transition-type", "grow"])?;
            Ok("Wallpaper set".into())
        }
        Some(Wallpaper::Hyprpaper) => {
            cmd::run("hyprctl", &["hyprpaper", "preload", &file])?;
            cmd::run("hyprctl", &["hyprpaper", "wallpaper", &format!(",{file}")])?;
            Ok("Wallpaper set".into())
        }
        _ => Err("No wallpaper tool found (end4-pC, swww or hyprpaper)".into()),
    }
}

/// Put an image on the clipboard as PNG.
pub fn copy_image(png: &[u8]) -> Result<(), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && cmd::exists("wl-copy") {
        clipboard::copy_bytes(png, "image/png")
    } else if cmd::exists("xclip") {
        spawn_with_stdin(
            "xclip",
            &["-selection", "clipboard", "-t", "image/png"],
            png,
        )
    } else {
        Err("Install wl-clipboard to copy images".into())
    }
}

/// xclip stays running to serve the clipboard, so don't wait for it to
/// exit.
fn spawn_with_stdin(program: &str, args: &[&str], input: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(input)
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Show the file's folder in the Material 3 file manager, or the default one.
pub fn open_folder(path: &Path) -> Result<(), String> {
    let dir = path.parent().unwrap_or(path).to_string_lossy().to_string();
    let local = paths::home().join(".local/bin/m3fm");
    if cmd::exists("m3fm") {
        cmd::spawn_detached("m3fm", &[&dir])
    } else if local.is_file() {
        cmd::spawn_detached(&local.to_string_lossy(), &[&dir])
    } else {
        cmd::spawn_detached("xdg-open", &[&dir])
    }
}

pub fn trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_backends_resolve_to_themselves() {
        assert_eq!(wallpaper_backend(Wallpaper::Swww), Some(Wallpaper::Swww));
        assert_eq!(
            wallpaper_backend(Wallpaper::Hyprpaper),
            Some(Wallpaper::Hyprpaper)
        );
    }
}
