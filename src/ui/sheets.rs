//! The side sheet: details, edits, preferences.

use super::{can_overwrite, edit_block_reason, slider};
use crate::app::{CropState, Sheet, ViewerApp};
use crate::config::{Wallpaper, Zoom};
use crate::edit::{Aspect, Crop};
use crate::loader::Slot;
use crate::media::human_size;
use egui::{vec2, Margin, RichText, Ui};
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::icons::MaterialIcon;
use material_3_ui::prefs;
use material_3_ui::widgets::{self, ButtonKind};

impl ViewerApp {
    pub(super) fn sheet_ui(&mut self, ui: &mut Ui) {
        let p = widgets::palette(ui);
        let (icon, title) = match self.sheet {
            Sheet::Info => (ICON_INFO, "Details"),
            Sheet::Edit => (ICON_TUNE, "Edit"),
            Sheet::Settings => (ICON_SETTINGS, "Preferences"),
            Sheet::None => return,
        };
        egui::Frame::new()
            .inner_margin(Margin {
                left: 20,
                right: 12,
                top: 14,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::icon(ui, icon, 22.0, p.primary);
                    ui.label(widgets::title_text(title, 18.0).color(p.on_surface));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, ICON_CLOSE, "Close (Esc)", true).clicked() {
                            self.sheet = Sheet::None;
                            self.crop = None;
                        }
                    });
                });
            });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin {
                        left: 16,
                        right: 16,
                        top: 0,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.sheet {
                            Sheet::Info => self.info_sheet(ui),
                            Sheet::Edit => self.edit_sheet(ui),
                            Sheet::Settings => self.settings_sheet(ui),
                            Sheet::None => {}
                        }
                    });
            });
    }

    fn info_sheet(&mut self, ui: &mut Ui) {
        let Some(path) = self.current().cloned() else {
            common::empty_hint(ui, "Nothing open.");
            return;
        };
        let p = widgets::palette(ui);
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(path.file_name().unwrap_or_default().to_string_lossy())
                    .size(15.0)
                    .color(p.on_surface),
            )
            .wrap(),
        );
        let meta = std::fs::metadata(&path).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok()).map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%b %-d, %Y at %H:%M")
                .to_string()
        });
        let folder = material_3_ui::sys::paths::tilde(path.parent().unwrap_or(&path));
        let mut facts: Vec<(&str, String)> = Vec::new();
        if self.is_video() {
            if let Some(pl) = &self.player {
                let st = &pl.state;
                facts.push(("Type", "Video".into()));
                if st.size[0] > 0 {
                    facts.push(("Resolution", format!("{} × {}", st.size[0], st.size[1])));
                }
                if st.duration > 0.0 {
                    facts.push(("Length", crate::video::format_time(st.duration)));
                }
                if !st.codec.is_empty() {
                    facts.push(("Codec", st.codec.clone()));
                }
                if st.fps > 0.0 {
                    facts.push((
                        "Frame rate",
                        format!("{:.3} fps", st.fps).replace(".000", ""),
                    ));
                }
                let count = |k: &str| pl.tracks.iter().filter(|t| t.kind == k).count();
                facts.push((
                    "Tracks",
                    format!("{} audio, {} subtitle", count("audio"), count("sub")),
                ));
            }
        } else if let Some(Slot::Ready(pic)) = self.current_slot() {
            let [w, h] = pic.full_size;
            facts.push(("Type", pic.format.clone()));
            facts.push((
                "Dimensions",
                format!("{w} × {h}  ({:.1} MP)", w as f64 * h as f64 / 1e6),
            ));
            if pic.animated() {
                let total: std::time::Duration = pic.frames.iter().map(|f| f.1).sum();
                facts.push((
                    "Animation",
                    format!("{} frames, {:.1} s", pic.frames.len(), total.as_secs_f32()),
                ));
            }
            let exif = pic.exif.clone();
            let [ow, oh] = self.edits.output_size(w, h);
            if !self.edits.is_identity() {
                facts.push(("After edits", format!("{ow} × {oh}")));
            }
            self.info_common(
                ui,
                &mut facts,
                meta.as_ref().map(|m| m.len()),
                modified,
                folder,
            );
            if !exif.is_empty() {
                card(ui, "Camera", |ui| {
                    let items: Vec<(&str, String)> =
                        exif.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                    common::facts(ui, "exif", &items);
                });
            }
            self.info_actions(ui);
            return;
        }
        self.info_common(
            ui,
            &mut facts,
            meta.as_ref().map(|m| m.len()),
            modified,
            folder,
        );
        self.info_actions(ui);
    }

    fn info_common(
        &mut self,
        ui: &mut Ui,
        facts: &mut Vec<(&str, String)>,
        size: Option<u64>,
        modified: Option<String>,
        folder: String,
    ) {
        if let Some(s) = size {
            facts.push(("Size", human_size(s)));
        }
        if let Some(m) = modified {
            facts.push(("Modified", m));
        }
        facts.push(("Folder", folder));
        card(ui, "File", |ui| common::facts(ui, "file-facts", facts));
    }

    fn info_actions(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::button(ui, ButtonKind::Tonal, Some(ICON_FOLDER), "Show in Files").clicked()
            {
                self.open_folder();
            }
            let video = self.is_video();
            let ok = crate::actions::wallpaper_backend(self.cfg.wallpaper)
                .is_some_and(|b| !video || b == Wallpaper::End4);
            if widgets::button_enabled(
                ui,
                ButtonKind::Tonal,
                Some(ICON_WALLPAPER),
                "Set as wallpaper",
                ok,
            )
            .clicked()
            {
                self.set_wallpaper(&ctx);
            }
        });
    }

    fn edit_sheet(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let p = widgets::palette(ui);
        if let Some(reason) = edit_block_reason(self) {
            ui.add_space(12.0);
            common::note(ui, reason);
            return;
        }
        let Some(pic) = self.picture() else { return };
        let [fw, fh] = pic.full_size;

        card(ui, "Rotate & flip", |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let tool = |ui: &mut Ui, icon: MaterialIcon, tip: &str| {
                    widgets::icon_button_sized(ui, icon, tip, true, 44.0, false).clicked()
                };
                if tool(ui, ICON_ROTATE_LEFT, "Rotate left (Shift+R)") {
                    self.edits.rotate_ccw();
                }
                if tool(ui, ICON_ROTATE_RIGHT, "Rotate right (R)") {
                    self.edits.rotate_cw();
                }
                if tool(ui, ICON_FLIP, "Flip horizontally") {
                    self.edits.flip_horizontal();
                }
                if tool(ui, ICON_SWAP_VERT, "Flip vertically") {
                    self.edits.flip_vertical();
                }
            });
            ui.add_space(4.0);
        });

        card(ui, "Crop", |ui| {
            ui.add_space(6.0);
            let oriented = self.edits.oriented_size(fw, fh);
            match &mut self.crop {
                None => {
                    ui.horizontal(|ui| {
                        if widgets::button(ui, ButtonKind::Tonal, Some(ICON_CROP), "Crop").clicked()
                        {
                            self.crop = Some(CropState {
                                aspect: Aspect::Free,
                                draft: self.edits.crop,
                            });
                        }
                        if !self.edits.crop.is_full()
                            && widgets::button(ui, ButtonKind::Text, None, "Reset").clicked()
                        {
                            self.edits.crop = Crop::FULL;
                        }
                    });
                    if !self.edits.crop.is_full() {
                        let [cw, ch] = self.edits.cropped_size(fw, fh);
                        common::note(ui, &format!("Cropped to {cw} × {ch}."));
                    }
                }
                Some(state) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for a in Aspect::ALL {
                            if widgets::filter_chip(ui, a.label(), state.aspect == a, None)
                                .clicked()
                            {
                                state.aspect = a;
                                state.draft = a.fit(state.draft, oriented[0], oriented[1]);
                            }
                        }
                    });
                    ui.add_space(8.0);
                    let mut apply = false;
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        if widgets::button(ui, ButtonKind::Filled, Some(ICON_CHECK), "Apply")
                            .clicked()
                        {
                            apply = true;
                        }
                        if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                            cancel = true;
                        }
                        if widgets::button(ui, ButtonKind::Text, None, "Reset").clicked() {
                            state.draft = state.aspect.fit(Crop::FULL, oriented[0], oriented[1]);
                        }
                    });
                    if apply {
                        self.edits.crop = state.draft;
                        self.edits.resize = None;
                        self.resize_text = [String::new(), String::new()];
                        self.crop = None;
                    } else if cancel {
                        self.crop = None;
                    }
                }
            }
            ui.add_space(4.0);
        });

        card(ui, "Resize", |ui| {
            let [cw, ch] = self.edits.cropped_size(fw, fh);
            let current = self.edits.resize.unwrap_or([cw, ch]);
            // Refill the fields when they're empty or the crop or rotation
            // changed the size they were based on.
            if self.resize_text[0].is_empty() || self.resize_basis != [cw, ch] {
                self.resize_text = [current[0].to_string(), current[1].to_string()];
                self.resize_basis = [cw, ch];
            }
            ui.add_space(6.0);
            let mut changed_w = false;
            let mut changed_h = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                changed_w = ui
                    .add(egui::TextEdit::singleline(&mut self.resize_text[0]).desired_width(64.0))
                    .changed();
                ui.label(RichText::new("×").color(p.on_surface_variant));
                changed_h = ui
                    .add(egui::TextEdit::singleline(&mut self.resize_text[1]).desired_width(64.0))
                    .changed();
                let lock_icon = if self.resize_lock {
                    ICON_LINK
                } else {
                    ICON_LINK_OFF
                };
                if widgets::icon_button_sized(
                    ui,
                    lock_icon,
                    "Keep proportions",
                    true,
                    32.0,
                    self.resize_lock,
                )
                .clicked()
                {
                    self.resize_lock = !self.resize_lock;
                }
            });
            let ratio = cw as f64 / ch.max(1) as f64;
            if self.resize_lock {
                if changed_w {
                    if let Ok(w) = self.resize_text[0].trim().parse::<f64>() {
                        self.resize_text[1] = ((w / ratio).round().max(1.0) as u32).to_string();
                    }
                } else if changed_h {
                    if let Ok(h) = self.resize_text[1].trim().parse::<f64>() {
                        self.resize_text[0] = ((h * ratio).round().max(1.0) as u32).to_string();
                    }
                }
            }
            let parsed = (
                self.resize_text[0]
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|v| (1..=65535).contains(v)),
                self.resize_text[1]
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|v| (1..=65535).contains(v)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for pct in [25u32, 50, 75] {
                    if widgets::filter_chip(ui, &format!("{pct}%"), false, None).clicked() {
                        self.resize_text = [
                            (cw * pct / 100).max(1).to_string(),
                            (ch * pct / 100).max(1).to_string(),
                        ];
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let target = match parsed {
                    (Some(w), Some(h)) => Some([w, h]),
                    _ => None,
                };
                let differs = target.is_some_and(|t| Some(t) != self.edits.resize && t != [cw, ch]);
                if widgets::button_enabled(
                    ui,
                    ButtonKind::Tonal,
                    Some(ICON_PHOTO_SIZE_SELECT_LARGE),
                    "Apply size",
                    differs,
                )
                .clicked()
                {
                    self.edits.resize = target;
                }
                if self.edits.resize.is_some()
                    && widgets::button(ui, ButtonKind::Text, None, "Original size").clicked()
                {
                    self.edits.resize = None;
                    self.resize_text = [cw.to_string(), ch.to_string()];
                }
            });
            if parsed.0.is_none() || parsed.1.is_none() {
                common::note(ui, "Enter a width and height in pixels.");
            } else if let Some([w, h]) = self.edits.resize {
                common::note(ui, &format!("Saves at {w} × {h}."));
            }
            ui.add_space(4.0);
        });

        card(ui, "Adjust", |ui| {
            ui.add_space(4.0);
            let adjust = |ui: &mut Ui, label: &str, icon: MaterialIcon, v: &mut f32| {
                ui.horizontal(|ui| {
                    widgets::icon(ui, icon, 18.0, p.on_surface_variant);
                    ui.label(RichText::new(label).color(p.on_surface));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if *v != 0.0
                            && widgets::icon_button_sized(
                                ui,
                                ICON_RESTART_ALT,
                                "Reset",
                                true,
                                28.0,
                                false,
                            )
                            .clicked()
                        {
                            *v = 0.0;
                        }
                        ui.label(
                            RichText::new(format!("{:+.0}", *v * 100.0))
                                .color(p.on_surface_variant)
                                .monospace(),
                        );
                    });
                });
                let mut x = *v as f64;
                if slider(ui, &mut x, -1.0..=1.0, ui.available_width()).changed() {
                    // Snap to zero near the middle.
                    *v = if x.abs() < 0.03 { 0.0 } else { x as f32 };
                }
                ui.add_space(4.0);
            };
            adjust(
                ui,
                "Brightness",
                ICON_LIGHT_MODE,
                &mut self.edits.brightness,
            );
            adjust(ui, "Contrast", ICON_CONTRAST, &mut self.edits.contrast);
            adjust(ui, "Saturation", ICON_PALETTE, &mut self.edits.saturation);
        });

        ui.add_space(20.0);
        let edited = !self.edits.is_identity();
        let overwrite = can_overwrite(self);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if widgets::button_enabled(
                ui,
                ButtonKind::Filled,
                Some(ICON_SAVE),
                "Save",
                edited && overwrite,
            )
            .on_hover_text("Ctrl+S")
            .clicked()
            {
                self.confirm_overwrite();
            }
            if widgets::button_enabled(
                ui,
                ButtonKind::Tonal,
                Some(ICON_SAVE_AS),
                "Save as…",
                edited,
            )
            .on_hover_text("Ctrl+Shift+S")
            .clicked()
            {
                self.save_as(&ctx);
            }
            if widgets::button_enabled(
                ui,
                ButtonKind::Text,
                Some(ICON_UNDO),
                "Reset all",
                edited || self.crop.is_some(),
            )
            .clicked()
            {
                self.edits = Default::default();
                self.crop = None;
                self.resize_text = [String::new(), String::new()];
            }
        });
        if edited && !overwrite {
            common::note(ui, "This format can't be written back, so use Save as.");
        }
        ui.add_space(4.0);
        common::note(ui, "Edits only change the file when you save.");
    }

    fn settings_sheet(&mut self, ui: &mut Ui) {
        let mut changed = false;
        if prefs::appearance_card(ui, &mut self.cfg.appearance, &self.theme) {
            self.theme.apply(&self.cfg.appearance);
            changed = true;
        }
        card(ui, "Viewing", |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Pictures open at").color(widgets::palette(ui).on_surface));
            ui.add_space(4.0);
            changed |= widgets::segmented(
                ui,
                &mut self.cfg.default_zoom,
                &[
                    (Zoom::Fit, ICON_FIT_SCREEN, "Fit"),
                    (Zoom::Fill, ICON_FULLSCREEN, "Fill"),
                    (Zoom::Actual, ICON_CROP_FREE, "100%"),
                ],
            );
            common::note(
                ui,
                match self.cfg.default_zoom {
                    Zoom::Fit => "Fit the window, but small pictures stay at their real size.",
                    Zoom::Fill => "Fit the window, enlarging small pictures too.",
                    Zoom::Actual => "One picture pixel per screen pixel.",
                },
            );
            row(
                ui,
                "Filmstrip",
                "Thumbnails of the folder under the picture.",
                |ui| {
                    changed |= widgets::switch(ui, &mut self.cfg.show_filmstrip).changed();
                },
            );
            row(ui, "Ask before trashing", "", |ui| {
                changed |= widgets::switch(ui, &mut self.cfg.confirm_trash).changed();
            });
            ui.add_space(4.0);
        });
        card(ui, "Slideshow", |ui| {
            ui.add_space(6.0);
            let p = widgets::palette(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Each picture for").color(p.on_surface));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0} s", self.cfg.slideshow_seconds))
                            .color(p.on_surface_variant),
                    );
                });
            });
            let mut secs = self.cfg.slideshow_seconds as f64;
            let r = slider(ui, &mut secs, 1.0..=30.0, ui.available_width());
            if r.changed() {
                self.cfg.slideshow_seconds = secs.round() as f32;
            }
            changed |= r.drag_stopped() || r.clicked();
            common::note(ui, "Videos play to the end before the next one.");
        });
        card(ui, "Videos", |ui| {
            row(ui, "Loop", "Start videos over when they end.", |ui| {
                if widgets::switch(ui, &mut self.cfg.loop_video).changed() {
                    changed = true;
                    if let Some(pl) = &self.player {
                        pl.set_loop(self.cfg.loop_video);
                    }
                }
            });
            let status = match &self.player {
                Some(_) => "Playing through libmpv, inside this window.".to_string(),
                None => format!(
                    "Unavailable: {}",
                    self.player_error
                        .clone()
                        .unwrap_or_else(|| "libmpv isn't installed".into())
                ),
            };
            common::note(ui, &status);
            ui.add_space(4.0);
        });
        card(ui, "Wallpaper", |ui| {
            ui.add_space(6.0);
            changed |= widgets::segmented(
                ui,
                &mut self.cfg.wallpaper,
                &[
                    (Wallpaper::Auto, ICON_AUTO_AWESOME, "Auto"),
                    (Wallpaper::End4, ICON_PALETTE, "end4-pC"),
                    (Wallpaper::Swww, ICON_WALLPAPER, "swww"),
                    (Wallpaper::Hyprpaper, ICON_WALLPAPER, "hyprpaper"),
                ],
            );
            let note = match crate::actions::wallpaper_backend(self.cfg.wallpaper) {
                Some(Wallpaper::End4) => "end4-pC's switchwall.sh: sets the wallpaper and recolors the desktop and these apps to match. Videos work too.".to_string(),
                Some(Wallpaper::Swww) => "swww img (needs swww-daemon running).".into(),
                Some(Wallpaper::Hyprpaper) => "hyprpaper through hyprctl.".into(),
                _ => "No wallpaper tool found. Install end4-pC, swww or hyprpaper.".into(),
            };
            common::note(ui, &note);
            ui.add_space(4.0);
        });
        if changed {
            self.save_config();
        }
    }
}
