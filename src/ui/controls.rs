//! Video controls: a floating Material bar over the bottom of the video.

use super::{check_item, slider};
use crate::app::ViewerApp;
use crate::video::format_time;
use egui::{pos2, vec2, CornerRadius, Rect, RichText, Ui, UiBuilder};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets;

const SPEEDS: [f64; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

impl ViewerApp {
    pub(super) fn video_controls(&mut self, ui: &mut Ui, rect: Rect) {
        let p = widgets::palette(ui);
        let loop_video = self.cfg.loop_video;
        let mut set_loop = None;
        let mut screenshot = false;
        let Some(player) = &mut self.player else {
            return;
        };
        if player.file.is_none() {
            return;
        }
        let width = (rect.width() - 32.0).min(760.0);
        let bar = Rect::from_center_size(
            pos2(rect.center().x, rect.bottom() - 16.0 - 46.0),
            vec2(width, 92.0),
        );
        ui.painter().rect_filled(
            bar,
            CornerRadius::same(28),
            p.surface_container_high.gamma_multiply(0.95),
        );
        // Clicks on the bar shouldn't reach the video underneath.
        let _ = ui.interact(
            bar,
            egui::Id::unique("video-bar"),
            egui::Sense::click_and_drag(),
        );
        let st = player.state.clone();
        ui.scope_builder(
            UiBuilder::new().max_rect(bar.shrink2(vec2(18.0, 8.0))),
            |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
                // Seek bar with times either side.
                ui.horizontal(|ui| {
                    let time = |t: f64| {
                        RichText::new(format_time(t))
                            .size(12.0)
                            .color(p.on_surface_variant)
                            .monospace()
                    };
                    ui.label(time(st.position));
                    let mut pos = st.position;
                    let w = ui.available_width() - 52.0;
                    if slider(ui, &mut pos, 0.0..=st.duration.max(0.001), w).changed() {
                        player.seek(pos);
                    }
                    ui.label(time(st.duration));
                });
                ui.horizontal(|ui| {
                    ui.set_height(44.0);
                    let play_icon = if st.paused || st.eof {
                        ICON_PLAY_ARROW
                    } else {
                        ICON_PAUSE
                    };
                    // A filled circle for the main action.
                    let (r, resp) =
                        ui.allocate_exact_size(egui::Vec2::splat(44.0), egui::Sense::click());
                    let fill = widgets::state_layer(p.primary, p.on_primary, &resp);
                    ui.painter().circle_filled(r.center(), 22.0, fill);
                    widgets::paint_icon(ui, play_icon, r.center(), 26.0, p.on_primary);
                    if resp.on_hover_text("Play / pause (Space)").clicked() {
                        player.toggle_pause();
                    }
                    if widgets::icon_button(ui, ICON_SKIP_PREVIOUS, "Previous frame (,)", true)
                        .clicked()
                    {
                        player.frame_step(false);
                    }
                    if widgets::icon_button(ui, ICON_SKIP_NEXT, "Next frame (.)", true).clicked() {
                        player.frame_step(true);
                    }
                    if widgets::icon_button(ui, ICON_REPLAY_10, "Back 10 s", true).clicked() {
                        player.seek_by(-10.0);
                    }
                    if widgets::icon_button(ui, ICON_FORWARD_10, "Forward 10 s", true).clicked() {
                        player.seek_by(10.0);
                    }
                    ui.add_space(6.0);
                    let vol_icon = if st.muted || st.volume <= 0.0 {
                        ICON_VOLUME_OFF
                    } else if st.volume < 50.0 {
                        ICON_VOLUME_DOWN
                    } else {
                        ICON_VOLUME_UP
                    };
                    if widgets::icon_button(ui, vol_icon, "Mute (M)", true).clicked() {
                        player.set("mute", if st.muted { "no" } else { "yes" });
                    }
                    let mut vol = st.volume;
                    if slider(ui, &mut vol, 0.0..=130.0, 90.0)
                        .on_hover_text(format!("{vol:.0}%"))
                        .changed()
                    {
                        player.set_f64("volume", vol);
                        if st.muted {
                            player.set("mute", "no");
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, ICON_PHOTO_CAMERA, "Save this frame", true)
                            .clicked()
                        {
                            screenshot = true;
                        }
                        if widgets::icon_button_sized(
                            ui,
                            ICON_REPEAT,
                            "Loop",
                            true,
                            36.0,
                            loop_video,
                        )
                        .clicked()
                        {
                            set_loop = Some(!loop_video);
                        }
                        let subs: Vec<_> = player
                            .tracks
                            .iter()
                            .filter(|t| t.kind == "sub")
                            .cloned()
                            .collect();
                        let audio: Vec<_> = player
                            .tracks
                            .iter()
                            .filter(|t| t.kind == "audio")
                            .cloned()
                            .collect();
                        let mut select = None;
                        ui.add_enabled_ui(!subs.is_empty(), |ui| {
                            ui.menu_button(
                                widgets::icon_text(ICON_SUBTITLES, 22.0)
                                    .color(p.on_surface_variant),
                                |ui| {
                                    ui.set_width(220.0);
                                    let none = !subs.iter().any(|t| t.selected);
                                    if check_item(ui, none, "Off").clicked() {
                                        select = Some(("sub", None));
                                    }
                                    for t in &subs {
                                        if check_item(ui, t.selected, &t.label).clicked() {
                                            select = Some(("sub", Some(t.id)));
                                        }
                                    }
                                },
                            )
                            .response
                            .on_hover_text("Subtitles");
                        });
                        ui.add_enabled_ui(audio.len() > 1, |ui| {
                            ui.menu_button(
                                widgets::icon_text(ICON_AUDIOTRACK, 22.0)
                                    .color(p.on_surface_variant),
                                |ui| {
                                    ui.set_width(220.0);
                                    for t in &audio {
                                        if check_item(ui, t.selected, &t.label).clicked() {
                                            select = Some(("audio", Some(t.id)));
                                        }
                                    }
                                },
                            )
                            .response
                            .on_hover_text("Audio track");
                        });
                        if let Some((kind, id)) = select {
                            player.select_track(kind, id);
                        }
                        let speed_label = RichText::new(format!("{}×", trim(st.speed)))
                            .color(p.on_surface_variant);
                        ui.menu_button(speed_label, |ui| {
                            for s in SPEEDS {
                                if check_item(
                                    ui,
                                    (s - st.speed).abs() < 0.01,
                                    &format!("{}×", trim(s)),
                                )
                                .clicked()
                                {
                                    player.set_f64("speed", s);
                                }
                            }
                        })
                        .response
                        .on_hover_text("Speed");
                        if st.buffering {
                            ui.add(egui::Spinner::new().size(16.0).color(p.primary));
                        }
                    });
                });
            },
        );
        if let Some(on) = set_loop {
            if let Some(player) = &self.player {
                player.set_loop(on);
            }
            self.cfg.loop_video = on;
            self.save_config();
        }
        if screenshot {
            self.frame_screenshot();
        }
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn speed_labels() {
        assert_eq!(super::trim(1.0), "1");
        assert_eq!(super::trim(0.25), "0.25");
        assert_eq!(super::trim(1.5), "1.5");
    }
}
