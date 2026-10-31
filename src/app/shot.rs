//! Self-screenshot for checking the layout: renders a scene, saves a PNG and exits.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use kage::blend::{AI, SHADOW};
use kage::shading::Image8;

use super::{App, Phase, Screen, MODELS_TAB};
use crate::i18n::Lang;

/// One wheel notch every this many frames.
const WHEEL_EVERY: u32 = 20;
/// On the canvas at any window size.
const WHEEL_POINTER: egui::Pos2 = egui::pos2(480.0, 320.0);

pub(super) struct Shot {
    out: PathBuf,
    frame: u32,
    /// Lines of a wheel notch sent over the canvas; the zoom per frame goes to `<out>.zoom.txt`.
    wheel: Option<f32>,
    zoom_trace: String,
    /// Take the shot at this frame, whatever is going on: shows work in progress.
    at_frame: Option<u32>,
    /// Waiting for a run to finish before taking the shot.
    auto: Option<String>,
    started: bool,
    done_frame: Option<u32>,
    /// Show only this layer (and this blend mode) before the shot.
    solo: Option<(usize, Option<usize>)>,
    /// After the run: change the strength (re-solve), switch the background, then export.
    resolve: Option<f32>,
    background: Option<bool>,
    export: bool,
}

impl App {
    pub(super) fn with_scene(mut self, ctx: &egui::Context) -> Self {
        let Ok(out) = std::env::var("KAGE_SHOT") else { return self };
        let solo = match std::env::var("KAGE_VIEW").as_deref() {
            Ok("multiply") => Some((SHADOW, Some(0))),
            Ok("normal") => Some((SHADOW, Some(7))),
            Ok("ai") => Some((AI, None)),
            Ok("compare") => {
                self.compare_on = true;
                None
            }
            _ => None,
        };
        self.shot = Some(Shot {
            out: out.into(),
            frame: 0,
            wheel: std::env::var("KAGE_WHEEL").ok().and_then(|v| v.parse().ok()),
            zoom_trace: String::new(),
            at_frame: std::env::var("KAGE_SHOT_FRAME").ok().and_then(|v| v.parse().ok()),
            auto: std::env::var("KAGE_AUTO").ok(),
            started: false,
            done_frame: None,
            solo,
            resolve: std::env::var("KAGE_RESOLVE").ok().and_then(|v| v.parse().ok()),
            background: std::env::var("KAGE_BG_AFTER").ok().map(|v| v == "1"),
            export: std::env::var("KAGE_EXPORT").is_ok(),
        });
        if std::env::var("KAGE_LANG").as_deref() == Ok("en") {
            self.lang = Lang::En;
        }
        if let Ok(seed) = std::env::var("KAGE_SEED") {
            self.seed = seed;
        }
        let scene = std::env::var("KAGE_SCENE").unwrap_or_default();
        if scene != "empty" {
            if let Ok(path) = std::env::var("KAGE_IMAGE") {
                self.open_path(ctx, Path::new(&path));
            }
        }
        match scene.as_str() {
            "advanced" => self.adv_open = true,
            "history" => self.right_tab = 1,
            "export" => self.export_open = true,
            "quit" => self.quit_open = true,
            "settings_models" => {
                self.screen = Screen::Settings;
                self.settings_tab = MODELS_TAB;
            }
            "settings_comfy" => self.screen = Screen::Settings,
            "settings_log" => {
                self.screen = Screen::Settings;
                self.settings_tab = 2;
            }
            _ => {}
        }
        self
    }

    /// Wheel notches as the window sends them, so the real scroll smoothing runs too.
    pub(super) fn shot_input(&self, raw: &mut egui::RawInput) {
        let Some((shot, lines)) = self.shot.as_ref().and_then(|shot| Some((shot, shot.wheel?))) else { return };
        raw.events.push(egui::Event::PointerMoved(WHEEL_POINTER));
        if shot.frame.is_multiple_of(WHEEL_EVERY) {
            raw.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, lines),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }

    pub(super) fn handle_shot(&mut self, ctx: &egui::Context) {
        let Some(mut shot) = self.shot.take() else { return };
        shot.frame += 1;
        ctx.request_repaint();
        if shot.wheel.is_some() {
            shot.zoom_trace += &format!("{} {:.3} {:.4}\n", shot.frame, ctx.input(|i| i.time), self.scale * ctx.pixels_per_point());
        }
        let mut start: Option<String> = None;
        let mut export = false;
        let take = match &shot.auto {
            None => shot.frame == 30,
            Some(auto) => {
                if !shot.started && self.image.is_some() && (auto == "extract" || (self.verifier.all_ok() && self.comfy.url().is_some())) {
                    shot.started = true;
                    start = Some(auto.clone());
                }
                let shown = self.entry(self.current).map(|e| (e.result.params.strength, e.result.params.background));
                let drawn = self.views.as_ref().is_none_or(|v| v.shown.is_some() && !v.busy());
                let settled = matches!(self.phase, Phase::Done | Phase::Failed)
                    && self.resolve.is_none()
                    && self.export_rx.is_none()
                    && !self.export_pending
                    && drawn
                    && (shown.is_none() || shown == Some((self.cfg.strength, self.cfg.bg_shadow)));
                if shot.done_frame.is_none() && settled {
                    if let Some(k) = shot.resolve.take() {
                        self.cfg.strength = k;
                    } else if let Some(background) = shot.background.take() {
                        self.cfg.bg_shadow = background;
                    } else if shot.export {
                        shot.export = false;
                        export = true;
                    } else {
                        shot.done_frame = Some(shot.frame);
                        if let Some(light) = std::env::var("KAGE_LIGHT").ok().and_then(|v| v.parse().ok()) {
                            self.cfg.light_dir = light;
                        }
                        if let Ok(offset) = std::env::var("KAGE_MOVE") {
                            let mut parts = offset.split(',').filter_map(|v| v.parse().ok());
                            self.set_move_mode(true);
                            self.move_shadow(parts.next().unwrap_or(0), parts.next().unwrap_or(0));
                        }
                        if let Some((layer, blend)) = shot.solo {
                            self.visible = std::array::from_fn(|i| i == layer);
                            self.selected_layer = layer;
                            if let Some(b) = blend {
                                self.cfg.blend = b;
                            }
                        }
                    }
                }
                shot.done_frame.is_some_and(|f| shot.frame == f + 30)
            }
        };
        if shot.at_frame.map_or(take, |frame| shot.frame == frame) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let events = ctx.input(|i| i.raw.events.clone());
        for event in events {
            if let egui::Event::Screenshot { image, .. } = event {
                let [w, h] = image.size;
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, image.as_raw().to_vec()) {
                    let _ = buf.save(&shot.out);
                }
                if shot.wheel.is_some() {
                    let _ = std::fs::write(shot.out.with_extension("zoom.txt"), &shot.zoom_trace);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.shot = Some(shot);
        if export {
            self.request_export(ctx);
        }
        match start.as_deref() {
            Some("extract") => {
                if let Ok(p) = std::env::var("KAGE_SHADED") {
                    self.cfg.bg_shadow = std::env::var("KAGE_BG").as_deref() != Ok("0");
                    if let Ok(img) = Image8::load(Path::new(&p)) {
                        self.start_run(ctx, Some(Arc::new(img)));
                    }
                }
            }
            Some(_) => self.start_run(ctx, None),
            None => {}
        }
    }
}
