//! Self-screenshot for checking the layout (see CLAUDE.md): renders a scene, saves a PNG and exits.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use kage::blend::{AI, SHADOW};
use kage::shading::Image8;

use super::{App, Phase, Screen};
use crate::i18n::Lang;

pub(super) struct Shot {
    out: PathBuf,
    frame: u32,
    /// Waiting for a run to finish before taking the shot.
    auto: Option<String>,
    started: bool,
    done_frame: Option<u32>,
    /// Show only this layer (and this blend mode) before the shot.
    solo: Option<(usize, Option<usize>)>,
    /// After the run: change the strength (re-solve), then export.
    resolve: Option<f32>,
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
            auto: std::env::var("KAGE_AUTO").ok(),
            started: false,
            done_frame: None,
            solo,
            resolve: std::env::var("KAGE_RESOLVE").ok().and_then(|v| v.parse().ok()),
            export: std::env::var("KAGE_EXPORT").is_ok(),
        });
        if std::env::var("KAGE_LANG").as_deref() == Ok("en") {
            self.lang = Lang::En;
        }
        let scene = std::env::var("KAGE_SCENE").unwrap_or_default();
        if scene != "empty" {
            if let Ok(path) = std::env::var("KAGE_IMAGE") {
                self.open_path(ctx, Path::new(&path));
            }
        }
        match scene.as_str() {
            "advanced" => self.adv_default_open = true,
            "history" => self.right_tab = 1,
            "export" => self.export_open = true,
            "quit" => self.quit_open = true,
            "settings_models" => self.screen = Screen::Settings,
            "settings_comfy" => {
                self.screen = Screen::Settings;
                self.settings_tab = 1;
            }
            "settings_log" => {
                self.screen = Screen::Settings;
                self.settings_tab = 2;
            }
            _ => {}
        }
        self
    }

    pub(super) fn handle_shot(&mut self, ctx: &egui::Context) {
        let Some(mut shot) = self.shot.take() else { return };
        shot.frame += 1;
        ctx.request_repaint();
        let mut start: Option<String> = None;
        let mut export = false;
        let take = match &shot.auto {
            None => shot.frame == 30,
            Some(auto) => {
                if !shot.started && self.image.is_some() && (auto == "extract" || (self.verifier.all_ok() && self.comfy.url().is_some())) {
                    shot.started = true;
                    start = Some(auto.clone());
                }
                let shown_strength = self.current.and_then(|id| self.history.iter().find(|e| e.id == id)).map(|e| e.result.params.strength);
                let drawn = self.views.as_ref().is_none_or(|v| v.shown.is_some() && !v.busy());
                let settled = matches!(self.phase, Phase::Done | Phase::Failed)
                    && self.resolve.is_none()
                    && self.export_rx.is_none()
                    && !self.export_pending
                    && drawn
                    && (shown_strength.is_none() || shown_strength == Some(self.cfg.strength));
                if shot.done_frame.is_none() && settled {
                    if let Some(k) = shot.resolve.take() {
                        self.cfg.strength = k;
                    } else if shot.export {
                        shot.export = false;
                        export = true;
                    } else {
                        shot.done_frame = Some(shot.frame);
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
        if take {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let events = ctx.input(|i| i.raw.events.clone());
        for event in events {
            if let egui::Event::Screenshot { image, .. } = event {
                let [w, h] = image.size;
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, image.as_raw().to_vec()) {
                    let _ = buf.save(&shot.out);
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
