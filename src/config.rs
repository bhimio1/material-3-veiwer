//! ~/.config/m3-viewer/config.toml

use material_3_ui::sys::arch::AurHelper;
use material_3_ui::theme::ThemeSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Wallpaper {
    /// end4-pC's switchwall.sh, else swww, else hyprpaper.
    #[default]
    Auto,
    End4,
    Swww,
    Hyprpaper,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Zoom {
    /// Fit the window, but never enlarge small images.
    #[default]
    Fit,
    /// Fit the window, enlarging small images too.
    Fill,
    /// One image pixel per screen pixel.
    Actual,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub appearance: ThemeSettings,
    pub default_zoom: Zoom,
    pub slideshow_seconds: f32,
    pub show_filmstrip: bool,
    pub wallpaper: Wallpaper,
    pub loop_video: bool,
    pub volume: f64,
    pub confirm_trash: bool,
    /// Terminal for installing missing tools; empty picks one.
    pub terminal: String,
    pub aur_helper: AurHelper,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: ThemeSettings::default(),
            default_zoom: Zoom::Fit,
            slideshow_seconds: 4.0,
            show_filmstrip: true,
            wallpaper: Wallpaper::Auto,
            loop_video: false,
            volume: 100.0,
            confirm_trash: true,
            terminal: String::new(),
            aur_helper: AurHelper::Auto,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        material_3_ui::sys::paths::config_home().join("m3-viewer/config.toml")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let cfg = Config {
            slideshow_seconds: 7.5,
            default_zoom: Zoom::Actual,
            wallpaper: Wallpaper::Swww,
            loop_video: true,
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), cfg);
    }

    #[test]
    fn missing_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(Config::load_from(&path), Config::default());
        std::fs::write(&path, "slideshow_seconds = 2.0\n").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.slideshow_seconds, 2.0);
        assert!(cfg.show_filmstrip);
    }
}
