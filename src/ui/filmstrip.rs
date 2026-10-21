//! The row of thumbnails under the picture.

use crate::app::ViewerApp;
use crate::media::{self, Kind};
use egui::{pos2, vec2, Align, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets;
use std::collections::HashSet;

const ITEM: f32 = 64.0;

impl ViewerApp {
    pub(super) fn filmstrip(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let p = widgets::palette(ui);
        let mut clicked = None;
        let mut visible = HashSet::new();
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    // Centre a short strip.
                    let total = self.files.len() as f32 * (ITEM + 8.0);
                    let pad = ((ui.available_width() - total) / 2.0).max(0.0);
                    ui.add_space(pad);
                    for i in 0..self.files.len() {
                        let selected = i == self.index;
                        let (r, resp) = ui.allocate_exact_size(vec2(ITEM, ITEM), Sense::click());
                        if selected && self.scroll_filmstrip {
                            resp.scroll_to_me(Some(Align::Center));
                        }
                        if ui.is_rect_visible(r) {
                            let path = self.files[i].clone();
                            visible.insert(path.clone());
                            let radius = CornerRadius::same(if selected { 18 } else { 12 });
                            let painter = ui.painter();
                            painter.rect_filled(r, radius, p.surface_container_high);
                            match self.loader.thumbnail(&path) {
                                Some(Some(tex)) => {
                                    let [w, h] = tex.size();
                                    let (w, h) = (w as f32, h as f32);
                                    // Centre-crop to a square.
                                    let uv = if w > h {
                                        let d = (1.0 - h / w) / 2.0;
                                        Rect::from_min_max(pos2(d, 0.0), pos2(1.0 - d, 1.0))
                                    } else {
                                        let d = (1.0 - w / h) / 2.0;
                                        Rect::from_min_max(pos2(0.0, d), pos2(1.0, 1.0 - d))
                                    };
                                    painter.add(
                                        egui::epaint::RectShape::filled(r, radius, Color32::WHITE)
                                            .with_texture(tex.id(), uv),
                                    );
                                }
                                Some(None) => {
                                    widgets::paint_icon(
                                        ui,
                                        ICON_BROKEN_IMAGE,
                                        r.center(),
                                        26.0,
                                        p.on_surface_variant,
                                    );
                                }
                                None => {}
                            }
                            if media::kind_of(&path) == Some(Kind::Video) {
                                let c = r.right_bottom() - vec2(14.0, 14.0);
                                ui.painter()
                                    .circle_filled(c, 10.0, Color32::from_black_alpha(160));
                                widgets::paint_icon(ui, ICON_PLAY_ARROW, c, 16.0, Color32::WHITE);
                            }
                            if selected {
                                ui.painter().rect_stroke(
                                    r,
                                    radius,
                                    Stroke::new(3.0, p.primary),
                                    StrokeKind::Inside,
                                );
                            } else if resp.hovered() {
                                ui.painter().rect_filled(
                                    r,
                                    radius,
                                    p.on_surface.gamma_multiply(0.08),
                                );
                            }
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            resp.on_hover_text(
                                path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                            );
                        }
                    }
                });
            });
        self.scroll_filmstrip = false;
        self.loader.keep_thumbnails(&visible);
        if let Some(i) = clicked {
            self.go_to(&ctx, i);
        }
    }
}
