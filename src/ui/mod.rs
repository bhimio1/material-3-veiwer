//! Drawing the window: top bar, canvas, filmstrip, side sheets, dialogs.

mod canvas;
mod controls;
mod filmstrip;
mod sheets;

use crate::app::{Confirm, Sheet, ViewerApp};
use crate::loader::Slot;
use crate::media::{self, human_size};
use egui::{vec2, Color32, CornerRadius, Frame, Margin, Rect, Response, RichText, Sense, Ui};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::icons::MaterialIcon;
use material_3_ui::widgets::{self, ButtonKind};
use std::ops::RangeInclusive;

impl ViewerApp {
    pub fn draw(&mut self, ui: &mut Ui) {
        let p = self.theme.palette().clone();
        let ctx = ui.ctx().clone();
        if self.fullscreen {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(Color32::BLACK))
                .show(ui, |ui| self.canvas(ui));
        } else {
            let surface = Frame::new().fill(p.surface);
            egui::Panel::top("bar")
                .frame(surface.inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 8,
                    bottom: 8,
                }))
                .show_separator_line(false)
                .show(ui, |ui| self.top_bar(ui));
            if self.cfg.show_filmstrip && self.files.len() > 1 {
                egui::Panel::bottom("filmstrip")
                    .frame(surface.inner_margin(Margin {
                        left: 12,
                        right: 12,
                        top: 0,
                        bottom: 10,
                    }))
                    .show_separator_line(false)
                    .resizable(false)
                    .exact_size(84.0)
                    .show(ui, |ui| self.filmstrip(ui));
            }
            if self.sheet != Sheet::None {
                // The shared Appearance card needs more room than the others.
                let width = if self.sheet == Sheet::Settings {
                    560.0
                } else {
                    348.0
                };
                egui::Panel::right("sheet")
                    .frame(surface.inner_margin(Margin {
                        left: 0,
                        right: 12,
                        top: 0,
                        bottom: 12,
                    }))
                    .show_separator_line(false)
                    .resizable(false)
                    .exact_size(width)
                    .show(ui, |ui| {
                        Frame::new()
                            .fill(p.surface_container_low)
                            .corner_radius(CornerRadius::same(24))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                self.sheet_ui(ui);
                            });
                    });
            }
            egui::CentralPanel::default()
                .frame(surface.inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 0,
                    bottom: 12,
                }))
                .show(ui, |ui| {
                    Frame::new()
                        .fill(p.surface_container_low)
                        .corner_radius(CornerRadius::same(24))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            self.canvas(ui);
                        });
                });
        }
        self.dialogs(&ctx);
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        let p = widgets::palette(ui);
        let ctx = ui.ctx().clone();
        let has_file = self.current().is_some();
        let video = self.is_video();
        ui.horizontal(|ui| {
            ui.set_height(48.0);
            if widgets::icon_button(ui, ICON_FOLDER_OPEN, "Open… (Ctrl+O)", true).clicked() {
                self.open_dialog(&ctx, false);
            }
            ui.add_space(4.0);
            // Title and details, truncated to whatever the actions leave.
            let actions_w = if has_file { 470.0 } else { 90.0 };
            let text_w = (ui.available_width() - actions_w).max(80.0);
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.add_space(4.0);
                let name = self
                    .current()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Viewer".into());
                let g = widgets::truncated_galley(
                    ui,
                    &name,
                    widgets::title_font(18.0),
                    p.on_surface,
                    text_w,
                );
                ui.label(g);
                let details = self.details_line();
                if !details.is_empty() {
                    let g = widgets::truncated_galley(
                        ui,
                        &details,
                        egui::FontId::proportional(12.5),
                        p.on_surface_variant,
                        text_w,
                    );
                    ui.label(g);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fs = if self.fullscreen {
                    ICON_FULLSCREEN_EXIT
                } else {
                    ICON_FULLSCREEN
                };
                if widgets::icon_button(ui, fs, "Fullscreen (F11)", has_file).clicked() {
                    self.set_fullscreen(&ctx, !self.fullscreen);
                }
                self.more_menu(ui);
                if !has_file {
                    return;
                }
                let sel = |s: Sheet, this: &Self| this.sheet == s;
                if widgets::icon_button_sized(
                    ui,
                    ICON_INFO,
                    "Details (I)",
                    true,
                    36.0,
                    sel(Sheet::Info, self),
                )
                .clicked()
                {
                    self.toggle_sheet(Sheet::Info);
                }
                if widgets::icon_button_sized(
                    ui,
                    ICON_TUNE,
                    "Edit (E)",
                    !video,
                    36.0,
                    sel(Sheet::Edit, self),
                )
                .clicked()
                {
                    self.toggle_sheet(Sheet::Edit);
                }
                if widgets::icon_button_sized(
                    ui,
                    ICON_SLIDESHOW,
                    "Slideshow",
                    self.files.len() > 1,
                    36.0,
                    self.slideshow.is_some(),
                )
                .clicked()
                {
                    self.toggle_slideshow(&ctx);
                }
                if widgets::icon_button(ui, ICON_ROTATE_RIGHT, "Rotate (R)", !video).clicked() {
                    self.edits.rotate_cw();
                }
                if widgets::icon_button(ui, ICON_DELETE, "Move to trash (Delete)", true).clicked() {
                    self.request_trash(&ctx);
                }
                if !video {
                    ui.add_space(8.0);
                    self.zoom_controls(ui);
                }
            });
        });
    }

    fn toggle_sheet(&mut self, sheet: Sheet) {
        self.sheet = if self.sheet == sheet {
            Sheet::None
        } else {
            sheet
        };
        if self.sheet != Sheet::Edit {
            self.crop = None;
        }
    }

    fn zoom_controls(&mut self, ui: &mut Ui) {
        let p = widgets::palette(ui);
        Frame::new()
            .fill(p.surface_container_high)
            .corner_radius(CornerRadius::same(20))
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if widgets::icon_button_sized(
                    ui,
                    ICON_FIT_SCREEN,
                    "Fit (F)",
                    true,
                    32.0,
                    self.zoom.is_none(),
                )
                .clicked()
                {
                    self.zoom = None;
                    self.offset = egui::Vec2::ZERO;
                }
                if widgets::icon_button_sized(ui, ICON_ADD, "Zoom in (+)", true, 32.0, false)
                    .clicked()
                {
                    self.pending_zoom = Some(1.25);
                }
                let label = format!("{:.0}%", self.shown_zoom * 100.0);
                let r = ui.add(
                    egui::Label::new(RichText::new(label).color(p.on_surface).size(13.0))
                        .sense(Sense::click()),
                );
                if r.on_hover_text("Actual size (0)").clicked() {
                    self.zoom = Some(1.0);
                    self.offset = egui::Vec2::ZERO;
                }
                if widgets::icon_button_sized(ui, ICON_REMOVE, "Zoom out (−)", true, 32.0, false)
                    .clicked()
                {
                    self.pending_zoom = Some(0.8);
                }
            });
    }

    fn more_menu(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let has_file = self.current().is_some();
        let video = self.is_video();
        let wallpaper_ok = crate::actions::wallpaper_backend(self.cfg.wallpaper)
            .is_some_and(|b| !video || b == crate::config::Wallpaper::End4);
        ui.menu_button(widgets::icon_text(ICON_MORE_VERT, 22.0), |ui| {
            ui.set_width(MENU_WIDTH);
            if menu_item(ui, ICON_FILE_OPEN, "Open file…", true).clicked() {
                self.open_dialog(&ctx, false);
            }
            if menu_item(ui, ICON_FOLDER_OPEN, "Open folder…", true).clicked() {
                self.open_dialog(&ctx, true);
            }
            ui.separator();
            if menu_item(
                ui,
                ICON_WALLPAPER,
                "Set as wallpaper",
                has_file && wallpaper_ok,
            )
            .clicked()
            {
                self.set_wallpaper(&ctx);
            }
            if menu_item(ui, ICON_CONTENT_COPY, "Copy image", has_file && !video).clicked() {
                self.copy_image(&ctx);
            }
            if menu_item(ui, ICON_FOLDER, "Show in Files", has_file).clicked() {
                self.open_folder();
            }
            if menu_item(ui, ICON_DELETE, "Move to trash", has_file).clicked() {
                self.request_trash(&ctx);
            }
            ui.separator();
            if menu_item(ui, ICON_SETTINGS, "Preferences", true).clicked() {
                self.sheet = Sheet::Settings;
            }
        });
    }

    /// "3 of 12 · 4032 × 3024 · 2.1 MB"
    fn details_line(&mut self) -> String {
        let Some(path) = self.current().cloned() else {
            return String::new();
        };
        let mut parts = Vec::new();
        if self.files.len() > 1 {
            parts.push(format!("{} of {}", self.index + 1, self.files.len()));
        }
        if self.is_video() {
            if let Some(pl) = &self.player {
                let [w, h] = pl.state.size;
                if w > 0 {
                    parts.push(format!("{w} × {h}"));
                }
                if pl.state.duration > 0.0 {
                    parts.push(crate::video::format_time(pl.state.duration));
                }
            }
            if let Ok(m) = std::fs::metadata(&path) {
                parts.push(human_size(m.len()));
            }
        } else if let Some(Slot::Ready(pic)) = self.current_slot() {
            let [w, h] = pic.full_size;
            parts.push(format!("{w} × {h}"));
            parts.push(human_size(pic.file_size));
            parts.push(pic.format.clone());
        }
        if !self.edits.is_identity() {
            parts.push("Edited".into());
        }
        parts.join("  ·  ")
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm.clone() else {
            return;
        };
        let p = self.theme.palette().clone();
        let (icon, title, body, action, danger) = match &confirm {
            Confirm::Trash(path) => (
                ICON_DELETE,
                "Move to trash?",
                format!(
                    "{} will be moved to the trash. You can restore it from there.",
                    file_name(path)
                ),
                "Move to trash",
                true,
            ),
            Confirm::Overwrite(path) => (
                ICON_SAVE,
                "Save over the original?",
                format!(
                    "{} will be replaced by the edited picture. Its camera details (EXIF) aren't kept. Use Save as to keep the original.",
                    file_name(path)
                ),
                "Save",
                false,
            ),
        };
        let mut close = false;
        let mut confirmed = false;
        let mut dont_ask = false;
        let r = common::modal(ctx, &p, "confirm", icon, title, |ui| {
            ui.set_max_width(380.0);
            ui.label(RichText::new(&body).color(p.on_surface_variant));
            if matches!(confirm, Confirm::Trash(_)) {
                ui.add_space(8.0);
                let mut ask = true;
                ui.horizontal(|ui| {
                    if widgets::switch(ui, &mut ask).changed() {
                        dont_ask = true;
                    }
                    ui.label(RichText::new("Always ask").color(p.on_surface));
                });
            }
            common::dialog_buttons(ui, |ui| {
                let kind = if danger {
                    ButtonKind::Danger
                } else {
                    ButtonKind::Filled
                };
                if widgets::button(ui, kind, None, action).clicked() {
                    confirmed = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    close = true;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                confirmed = true;
            }
        });
        if dont_ask {
            self.cfg.confirm_trash = false;
            self.save_config();
            self.toasts
                .info("Won't ask again — turn it back on in Preferences");
        }
        if r.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if confirmed {
            self.confirm = None;
            match confirm {
                Confirm::Trash(path) => self.trash(ctx, path),
                Confirm::Overwrite(path) => self.save_to(ctx, path),
            }
        } else if close {
            self.confirm = None;
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

const MENU_WIDTH: f32 = 220.0;

/// A menu entry with a leading icon.
fn menu_item(ui: &mut Ui, icon: MaterialIcon, label: &str, enabled: bool) -> Response {
    let p = widgets::palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new((
            widgets::icon_text(icon, 18.0).color(p.on_surface_variant),
            RichText::new(label).color(p.on_surface),
        ))
        .frame(false)
        .min_size(vec2(MENU_WIDTH, 32.0)),
    )
}

/// A menu entry with a check mark when selected.
fn check_item(ui: &mut Ui, checked: bool, label: &str) -> Response {
    let p = widgets::palette(ui);
    let mark = if checked {
        p.primary
    } else {
        Color32::TRANSPARENT
    };
    ui.add(
        egui::Button::new((
            widgets::icon_text(ICON_CHECK, 18.0).color(mark),
            RichText::new(label).color(p.on_surface),
        ))
        .frame(false)
        .min_size(vec2(MENU_WIDTH, 32.0)),
    )
}

/// A thin M3 slider. Returns a response that's `changed()` while dragged.
pub fn slider(ui: &mut Ui, value: &mut f64, range: RangeInclusive<f64>, width: f32) -> Response {
    let p = widgets::palette(ui);
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let span = (hi - lo).max(f64::EPSILON);
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
            let v = lo + t * span;
            if v != *value {
                *value = v;
                response.mark_changed();
            }
        }
    }
    if ui.is_rect_visible(rect) {
        let t = ((*value - lo) / span).clamp(0.0, 1.0) as f32;
        let x = rect.left() + t * rect.width();
        let y = rect.center().y;
        let track = |a: f32, b: f32, c: Color32| {
            ui.painter().rect_filled(
                Rect::from_min_max(egui::pos2(a, y - 2.0), egui::pos2(b, y + 2.0)),
                CornerRadius::same(2),
                c,
            );
        };
        track(rect.left(), rect.right(), p.surface_container_highest);
        track(rect.left(), x, p.primary);
        let r = if response.hovered() || response.dragged() {
            8.0
        } else {
            6.5
        };
        ui.painter().circle_filled(egui::pos2(x, y), r, p.primary);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Whether a picture can be edited (and how to say why not).
pub fn edit_block_reason(app: &mut ViewerApp) -> Option<&'static str> {
    match app.current_kind() {
        None => Some("Open a picture to edit it."),
        Some(k) if k.is_video() => Some("Videos can't be edited here."),
        _ => match app.current_slot() {
            Some(Slot::Ready(pic)) if pic.animated() => {
                Some("Animated pictures can't be edited — saving would keep only the first frame.")
            }
            Some(Slot::Ready(_)) => None,
            Some(Slot::Failed(_)) => Some("This picture couldn't be opened."),
            _ => Some("Loading…"),
        },
    }
}

pub fn can_overwrite(app: &ViewerApp) -> bool {
    app.current()
        .is_some_and(|p| media::can_save(p) && media::kind_of(p) == Some(media::Kind::Image))
}
