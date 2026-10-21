//! The picture (or video) itself: fit, zoom, pan, animation, crop overlay,
//! previous/next buttons and the empty state.

use crate::app::ViewerApp;
use crate::config::Zoom;
use crate::edit::Crop;
use crate::loader::Slot;
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};
use std::time::Duration;

const FULLSCREEN_IDLE: Duration = Duration::from_millis(2500);

impl ViewerApp {
    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let ctx = ui.ctx().clone();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let p = widgets::palette(ui);

        if self.current().is_none() {
            self.empty_state(ui, rect);
        } else if self.is_video() {
            self.video_view(ui, rect, &response);
        } else {
            match self.current_slot() {
                Some(Slot::Ready(_)) => self.picture_view(ui, rect, &response),
                Some(Slot::Failed(e)) => {
                    let e = e.clone();
                    centered_message(ui, rect, ICON_BROKEN_IMAGE, "Can't open this picture", &e);
                }
                _ => {
                    let spinner = Rect::from_center_size(rect.center(), Vec2::splat(36.0));
                    ui.put(spinner, egui::Spinner::new().size(36.0).color(p.primary));
                }
            }
        }

        // Previous / next, on hover.
        let active = self.last_activity.elapsed() < FULLSCREEN_IDLE;
        if self.files.len() > 1
            && self.crop.is_none()
            && (response.hovered() || ui.rect_contains_pointer(rect))
            && (!self.fullscreen || active)
        {
            let mid = rect.center().y;
            for (dx, icon, tip, delta) in [
                (rect.left() + 36.0, ICON_CHEVRON_LEFT, "Previous (←)", -1),
                (rect.right() - 36.0, ICON_CHEVRON_RIGHT, "Next (→)", 1),
            ] {
                let r = Rect::from_center_size(pos2(dx, mid), Vec2::splat(48.0));
                ui.painter().circle_filled(
                    r.center(),
                    24.0,
                    p.surface_container_highest.gamma_multiply(0.9),
                );
                let clicked = ui
                    .scope_builder(UiBuilder::new().max_rect(r), |ui| {
                        widgets::icon_button_sized(ui, icon, tip, true, 48.0, false).clicked()
                    })
                    .inner;
                if clicked {
                    self.step(&ctx, delta);
                }
            }
        }

        if self.fullscreen && (active || self.slideshow.is_some() && active) {
            self.fullscreen_bar(ui, rect);
        }
        if self.fullscreen && !active {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        } else if self.fullscreen {
            ctx.request_repaint_after(FULLSCREEN_IDLE);
        }

        // Drop target highlight.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                CornerRadius::same(24),
                p.primary_container.gamma_multiply(0.85),
            );
            painter.rect_stroke(
                rect.shrink(6.0),
                CornerRadius::same(20),
                Stroke::new(2.0, p.primary),
                StrokeKind::Inside,
            );
            widgets::paint_icon(
                ui,
                ICON_DOWNLOAD,
                rect.center() - vec2(0.0, 24.0),
                44.0,
                p.on_primary_container,
            );
            painter.text(
                rect.center() + vec2(0.0, 22.0),
                Align2::CENTER_CENTER,
                "Drop to open",
                widgets::title_font(18.0),
                p.on_primary_container,
            );
        }
    }

    fn picture_view(&mut self, ui: &mut Ui, rect: Rect, response: &egui::Response) {
        let ctx = ui.ctx().clone();
        let Some(pic) = self.picture() else { return };
        let ppp = ctx.pixels_per_point();
        let full = pic.full_size;

        // What's shown: the edit preview when there is one, else the picture.
        let preview = self.preview.as_ref().filter(|pv| pv.key.path == pic.path);
        let (texture_id, logical, repaint_in) = match preview {
            Some(pv) => {
                let size = if pv.key.with_crop {
                    pv.key.edits.cropped_size(full[0], full[1])
                } else {
                    pv.key.edits.oriented_size(full[0], full[1])
                };
                (pv.texture.id(), size, None)
            }
            None => {
                let (tex, left) = pic.frame_at(self.anim_start.elapsed());
                (tex.id(), full, (left != Duration::MAX).then_some(left))
            }
        };
        if let Some(left) = repaint_in {
            ctx.request_repaint_after(left);
        }
        let img = vec2(logical[0].max(1) as f32, logical[1].max(1) as f32);
        let area = rect.shrink(if self.fullscreen { 0.0 } else { 16.0 });
        let fit = (area.width() / img.x).min(area.height() / img.y);
        let default_scale = match self.cfg.default_zoom {
            Zoom::Fit => fit.min(1.0 / ppp),
            Zoom::Fill => fit,
            Zoom::Actual => 1.0 / ppp,
        };
        let cropping = self.crop.is_some();
        if cropping {
            self.zoom = None;
            self.offset = Vec2::ZERO;
        }
        let mut scale = if cropping {
            fit
        } else {
            self.zoom.map(|z| z / ppp).unwrap_or(default_scale)
        };

        // Zoom: keyboard/buttons around the centre, wheel and pinch around the pointer.
        let zoom_to = |new_scale: f32, anchor: Pos2, offset: &mut Vec2, scale: &mut f32| {
            let new_scale = (new_scale * ppp).clamp(0.02, 32.0) / ppp;
            let rel = anchor - area.center();
            *offset = rel - (rel - *offset) * (new_scale / *scale);
            *scale = new_scale;
        };
        if let Some(factor) = self.pending_zoom.take() {
            if !cropping {
                zoom_to(scale * factor, area.center(), &mut self.offset, &mut scale);
                self.zoom = Some(scale * ppp);
            }
        }
        if !cropping && response.hovered() {
            let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-4 {
                if let Some(pointer) = response.hover_pos() {
                    zoom_to(scale * factor, pointer, &mut self.offset, &mut scale);
                    self.zoom = Some(scale * ppp);
                }
            }
            if response.double_clicked() {
                if self.zoom.is_some() {
                    self.zoom = None;
                    self.offset = Vec2::ZERO;
                    scale = default_scale;
                } else if let Some(pointer) = response.interact_pointer_pos() {
                    zoom_to(1.0 / ppp, pointer, &mut self.offset, &mut scale);
                    self.zoom = Some(1.0);
                }
            }
        }
        let size = img * scale;
        if !cropping && response.dragged_by(egui::PointerButton::Primary) {
            self.offset += response.drag_delta();
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if !cropping
            && (size.x > area.width() || size.y > area.height())
            && response.hovered()
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        let max = ((size - area.size()) / 2.0).max(Vec2::ZERO);
        self.offset = self.offset.clamp(-max, max);
        self.shown_zoom = scale * ppp;

        // Snap to whole physical pixels so 100% is crisp.
        let center = area.center() + self.offset;
        let min = ((center - size / 2.0) * ppp).round() / ppp;
        let image_rect = Rect::from_min_size(min, size);
        let painter = ui.painter_at(rect);
        if pic.image.color().has_alpha() {
            checkerboard(
                &painter,
                image_rect.intersect(rect),
                self.fullscreen || widgets::palette(ui).is_dark(),
            );
        }
        painter.image(
            texture_id,
            image_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        if cropping {
            let oriented = self.edits.oriented_size(full[0], full[1]);
            self.crop_overlay(ui, image_rect, oriented);
        }
    }

    fn crop_overlay(&mut self, ui: &mut Ui, image_rect: Rect, size: [u32; 2]) {
        let p = widgets::palette(ui);
        let Some(state) = &mut self.crop else { return };
        let to_screen = |c: &Crop| {
            Rect::from_min_size(
                image_rect.min + vec2(c.x * image_rect.width(), c.y * image_rect.height()),
                vec2(c.w * image_rect.width(), c.h * image_rect.height()),
            )
        };
        let ratio = state
            .aspect
            .ratio(size[0], size[1])
            .map(|r| r * size[1] as f32 / size[0].max(1) as f32);

        // Corners resize (the opposite corner stays put); the inside moves.
        let r = to_screen(&state.draft);
        let corners = [
            r.left_top(),
            r.right_top(),
            r.left_bottom(),
            r.right_bottom(),
        ];
        let mut changed = None;
        for (i, corner) in corners.iter().enumerate() {
            let hit = Rect::from_center_size(*corner, Vec2::splat(28.0));
            let resp = ui.interact(hit, egui::Id::unique(("crop-corner", i)), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if i == 0 || i == 3 {
                    egui::CursorIcon::ResizeNwSe
                } else {
                    egui::CursorIcon::ResizeNeSw
                });
            }
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let fx = ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
                    let fy = ((pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
                    let c = state.draft;
                    let (ax, ay) = match i {
                        0 => (c.x + c.w, c.y + c.h),
                        1 => (c.x, c.y + c.h),
                        2 => (c.x + c.w, c.y),
                        _ => (c.x, c.y),
                    };
                    let mut w = (fx - ax).abs().max(0.02);
                    let mut h = (fy - ay).abs().max(0.02);
                    if let Some(fr) = ratio {
                        // Keep the ratio, within the image.
                        let room_w = if fx < ax { ax } else { 1.0 - ax };
                        let room_h = if fy < ay { ay } else { 1.0 - ay };
                        h = w / fr;
                        if h > room_h {
                            h = room_h;
                            w = h * fr;
                        }
                        if w > room_w {
                            w = room_w;
                            h = w / fr;
                        }
                    }
                    let x = if fx < ax { ax - w } else { ax };
                    let y = if fy < ay { ay - h } else { ay };
                    changed = Some(Crop { x, y, w, h });
                }
            }
        }
        if changed.is_none() {
            let inner = r.shrink(14.0);
            if inner.is_positive() {
                let resp = ui.interact(inner, egui::Id::unique("crop-move"), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                }
                if resp.dragged() {
                    let d = resp.drag_delta();
                    let mut c = state.draft;
                    c.x += d.x / image_rect.width();
                    c.y += d.y / image_rect.height();
                    changed = Some(c);
                }
            }
        }
        if let Some(c) = changed {
            state.draft = c.clamped();
        }

        let r = to_screen(&state.draft);
        let painter = ui.painter();
        let scrim = Color32::from_black_alpha(140);
        for shade in [
            Rect::from_min_max(image_rect.min, pos2(image_rect.right(), r.top())),
            Rect::from_min_max(pos2(image_rect.left(), r.bottom()), image_rect.max),
            Rect::from_min_max(pos2(image_rect.left(), r.top()), pos2(r.left(), r.bottom())),
            Rect::from_min_max(
                pos2(r.right(), r.top()),
                pos2(image_rect.right(), r.bottom()),
            ),
        ] {
            if shade.is_positive() {
                painter.rect_filled(shade, CornerRadius::ZERO, scrim);
            }
        }
        let thin = Stroke::new(1.0, Color32::from_white_alpha(110));
        for t in [1.0 / 3.0, 2.0 / 3.0] {
            let x = r.left() + r.width() * t;
            let y = r.top() + r.height() * t;
            painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], thin);
            painter.line_segment([pos2(r.left(), y), pos2(r.right(), y)], thin);
        }
        painter.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(2.0, p.primary),
            StrokeKind::Outside,
        );
        for corner in [
            r.left_top(),
            r.right_top(),
            r.left_bottom(),
            r.right_bottom(),
        ] {
            painter.circle(corner, 7.0, p.primary, Stroke::new(2.0, p.on_primary));
        }
        let (_, _, pw, ph) = state.draft.pixels(size[0], size[1]);
        let label = format!("{pw} × {ph}");
        let galley = painter.layout_no_wrap(
            label,
            egui::FontId::proportional(12.0),
            p.on_secondary_container,
        );
        let tag = Rect::from_center_size(
            pos2(
                r.center().x,
                (r.bottom() + 18.0).min(image_rect.bottom() - 14.0),
            ),
            galley.size() + vec2(16.0, 8.0),
        );
        painter.rect_filled(tag, CornerRadius::same(10), p.secondary_container);
        painter.galley(
            tag.center() - galley.size() / 2.0,
            galley,
            p.on_secondary_container,
        );
    }

    fn video_view(&mut self, ui: &mut Ui, rect: Rect, response: &egui::Response) {
        let ctx = ui.ctx().clone();
        let p = widgets::palette(ui);
        let Some(player) = &mut self.player else {
            let reason = self.player_error.clone().unwrap_or_default();
            let installed = self
                .installed
                .get_or_insert_with(material_3_ui::sys::arch::installed_packages)
                .clone();
            let mut install = false;
            let mut external = false;
            ui.scope_builder(UiBuilder::new().max_rect(rect.shrink(32.0)), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((rect.height() / 2.0 - 140.0).max(0.0));
                    widgets::icon(ui, ICON_MOVIE, 52.0, p.primary);
                    ui.add_space(8.0);
                    ui.label(
                        widgets::title_text("Videos play through mpv", 20.0).color(p.on_surface),
                    );
                    ui.add_space(12.0);
                    ui.set_max_width(520.0);
                    if common::require(
                        ui,
                        &installed,
                        "mpv",
                        "mpv",
                        "Install it to play videos here.",
                    ) == common::Requirement::InstallRequested
                    {
                        install = true;
                    } else if material_3_ui::sys::cmd::exists("mpv") {
                        common::banner(
                            ui,
                            ICON_WARNING,
                            &format!("The video player couldn't start: {reason}"),
                            true,
                            |ui| {
                                if widgets::button(
                                    ui,
                                    ButtonKind::Tonal,
                                    Some(ICON_OPEN_IN_NEW),
                                    "Open in mpv",
                                )
                                .clicked()
                                {
                                    external = true;
                                }
                            },
                        );
                    }
                });
            });
            if install {
                self.install_mpv();
            }
            if external {
                if let Some(path) = self.current() {
                    let _ = material_3_ui::sys::cmd::spawn_detached(
                        "mpv",
                        &[
                            "--player-operation-mode=pseudo-gui",
                            "--",
                            &path.to_string_lossy(),
                        ],
                    );
                }
            }
            return;
        };
        let [w, h] = player.state.size;
        let area = rect.shrink(if self.fullscreen { 0.0 } else { 16.0 });
        if w > 0 && h > 0 {
            let img = vec2(w as f32, h as f32);
            let scale = (area.width() / img.x).min(area.height() / img.y);
            let ppp = ctx.pixels_per_point();
            let size = ((img * scale) * ppp).round() / ppp;
            let min = ((area.center() - size / 2.0) * ppp).round() / ppp;
            let video_rect = Rect::from_min_size(min, size);
            if !self.fullscreen {
                ui.painter()
                    .rect_filled(video_rect, CornerRadius::ZERO, Color32::BLACK);
            }
            ui.painter().add(player.paint_callback(video_rect));
            self.shown_zoom = scale * ppp;
        } else if player.file.is_some() {
            let spinner = Rect::from_center_size(rect.center(), Vec2::splat(36.0));
            ui.put(spinner, egui::Spinner::new().size(36.0).color(p.primary));
        }
        if response.double_clicked() {
            player.toggle_pause();
            self.set_fullscreen(&ctx, !self.fullscreen);
        } else if response.clicked() {
            player.toggle_pause();
        }
        let show_controls = !self.fullscreen
            || self.last_activity.elapsed() < FULLSCREEN_IDLE
            || self.player.as_ref().is_some_and(|p| p.state.paused);
        // Lift subtitles above the controls while they're showing.
        if show_controls != self.controls_shown {
            self.controls_shown = show_controls;
            if let Some(pl) = &self.player {
                pl.set("sub-pos", if show_controls { "84" } else { "100" });
            }
        }
        if show_controls {
            self.video_controls(ui, rect);
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let p = widgets::palette(ui);
        ui.scope_builder(UiBuilder::new().max_rect(rect.shrink(32.0)), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() / 2.0 - 130.0).max(0.0));
                let circle = ui.allocate_exact_size(Vec2::splat(96.0), Sense::hover()).0;
                ui.painter()
                    .circle_filled(circle.center(), 48.0, p.primary_container);
                widgets::paint_icon(
                    ui,
                    ICON_PHOTO_LIBRARY,
                    circle.center(),
                    46.0,
                    p.on_primary_container,
                );
                ui.add_space(16.0);
                ui.label(widgets::title_text("Open a picture or video", 24.0).color(p.on_surface));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Or drop one here. Pictures, animations, SVG, RAW and videos.",
                    )
                    .color(p.on_surface_variant),
                );
                ui.add_space(20.0);
                let w = 300.0;
                ui.allocate_ui_with_layout(
                    vec2(w, 40.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if widgets::button(
                            ui,
                            ButtonKind::Filled,
                            Some(ICON_FILE_OPEN),
                            "Open file",
                        )
                        .clicked()
                        {
                            self.open_dialog(&ctx, false);
                        }
                        if widgets::button(
                            ui,
                            ButtonKind::Tonal,
                            Some(ICON_FOLDER_OPEN),
                            "Open folder",
                        )
                        .clicked()
                        {
                            self.open_dialog(&ctx, true);
                        }
                    },
                );
            });
        });
    }

    fn fullscreen_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let p = widgets::palette(ui);
        let name = self
            .current()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let width = 520.0f32.min(rect.width() - 32.0);
        let bar =
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 40.0), vec2(width, 52.0));
        ui.painter().rect_filled(
            bar,
            CornerRadius::same(26),
            p.surface_container_high.gamma_multiply(0.94),
        );
        ui.scope_builder(
            UiBuilder::new().max_rect(bar.shrink2(vec2(12.0, 8.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    let counter = if self.files.len() > 1 {
                        format!("  {} / {}", self.index + 1, self.files.len())
                    } else {
                        String::new()
                    };
                    let text_w = ui.available_width() - 130.0;
                    let g = widgets::truncated_galley(
                        ui,
                        &format!("{name}{counter}"),
                        egui::FontId::proportional(14.0),
                        p.on_surface,
                        text_w,
                    );
                    ui.add_space(6.0);
                    ui.label(g);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(
                            ui,
                            ICON_FULLSCREEN_EXIT,
                            "Exit fullscreen (Esc)",
                            true,
                        )
                        .clicked()
                        {
                            self.slideshow = None;
                            self.set_fullscreen(&ctx, false);
                        }
                        let (icon, tip) = if self.slideshow.is_some() {
                            (ICON_PAUSE, "Stop slideshow")
                        } else {
                            (ICON_SLIDESHOW, "Slideshow")
                        };
                        if self.files.len() > 1
                            && widgets::icon_button(ui, icon, tip, true).clicked()
                        {
                            if self.slideshow.is_some() {
                                self.slideshow = None;
                            } else {
                                self.toggle_slideshow(&ctx);
                            }
                        }
                        if widgets::icon_button_sized(ui, ICON_INFO, "Details", true, 36.0, false)
                            .clicked()
                        {
                            self.set_fullscreen(&ctx, false);
                            self.sheet = crate::app::Sheet::Info;
                        }
                    });
                });
            },
        );
    }
}

fn centered_message(
    ui: &mut Ui,
    rect: Rect,
    icon: material_3_ui::icons::MaterialIcon,
    title: &str,
    detail: &str,
) {
    let p = widgets::palette(ui);
    ui.scope_builder(UiBuilder::new().max_rect(rect.shrink(32.0)), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((rect.height() / 2.0 - 80.0).max(0.0));
            widgets::icon(ui, icon, 48.0, p.on_surface_variant);
            ui.add_space(8.0);
            ui.label(widgets::title_text(title, 18.0).color(p.on_surface));
            ui.add(
                egui::Label::new(egui::RichText::new(detail).color(p.on_surface_variant)).wrap(),
            );
        });
    });
}

/// The usual grey checks behind transparent pictures.
fn checkerboard(painter: &egui::Painter, rect: Rect, dark: bool) {
    if !rect.is_positive() {
        return;
    }
    let (a, b) = if dark {
        (Color32::from_gray(40), Color32::from_gray(56))
    } else {
        (Color32::from_gray(204), Color32::from_gray(236))
    };
    painter.rect_filled(rect, CornerRadius::ZERO, a);
    let step = 12.0;
    let cols = (rect.width() / step).ceil() as i32;
    let rows = (rect.height() / step).ceil() as i32;
    // Very large areas: a flat colour is fine and far cheaper.
    if (cols as i64) * (rows as i64) > 40_000 {
        return;
    }
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                continue;
            }
            let cell = Rect::from_min_size(
                rect.min + vec2(x as f32 * step, y as f32 * step),
                Vec2::splat(step),
            )
            .intersect(rect);
            painter.rect_filled(cell, CornerRadius::ZERO, b);
        }
    }
}
