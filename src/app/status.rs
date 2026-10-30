//! Status bar: what is running, how far it is, what went wrong, and the view tools. Plain on purpose: one line of text
//! and, during a run, one progress bar.

use eframe::egui::{self, Align, Layout, RichText, Sense, Vec2};
use egui_phosphor::regular as ph;

use super::canvas::ZOOM_STEP;
use super::logic::failure_text;
use super::settings::open_in_explorer;
use super::{App, Phase, Screen};
use crate::theme::{BLUE, GREEN, LINE, MUTED, RED, SIZE_ICON, STATE_OFF};

/// Progress fraction where the AI stage ends; the rest is subject mask, shadow layer and finishing.
const AI_SHARE: f64 = 0.85;
/// Rough seconds after the last step (decode, subject mask, shadow layer) for the remaining-time estimate.
const AFTER_STEPS_SECONDS: f64 = 6.0;
const STAGES: usize = 4;

fn clock(sec: f64) -> String {
    let s = sec.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl App {
    fn progress(&self) -> f64 {
        match self.stage {
            0 if self.steps.1 > 0 => AI_SHARE * self.steps.0 as f64 / self.steps.1 as f64,
            0 => 0.0,
            1 => 0.88,
            2 => 0.95,
            _ => 0.99,
        }
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal_centered(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  {}", ph::LIST, t.log)).on_hover_text(t.log_tip).clicked() {
                    self.screen = Screen::Settings;
                    self.settings_tab = 2;
                }
                if self.screen == Screen::Main {
                    ui.separator();
                    self.view_tools(ui);
                }
                if let (Some(dir), false) = (&self.last_export_dir, self.running()) {
                    if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_folder)).on_hover_text(dir.display().to_string()).clicked() {
                        open_in_explorer(dir);
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| self.status_text(ui));
            });
        });
    }

    /// Compare and zoom (laid out right to left).
    fn view_tools(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let has_result = self.current.is_some();
        if ui.button("1:1").on_hover_text(t.actual_tip).clicked() {
            self.zoom_actual(&ctx);
        }
        if ui.button(t.fit).on_hover_text(t.fit_tip).clicked() {
            self.zoom_fit();
        }
        if ui.button("+").on_hover_text(t.zoom_in).clicked() {
            self.zoom_by(ZOOM_STEP);
        }
        let percent = if self.image.is_some() { format!("{:>4.0}%", self.scale * ctx.pixels_per_point() * 100.0) } else { "   -%".to_string() };
        ui.label(RichText::new(percent).monospace());
        if ui.button("−").on_hover_text(t.zoom_out).clicked() {
            self.zoom_by(1.0 / ZOOM_STEP);
        }
        ui.separator();
        let compare = egui::Button::new(format!("{}  {}", ph::SQUARE_SPLIT_HORIZONTAL, t.compare)).selected(has_result && self.compare_on);
        if ui.add_enabled(has_result, compare).on_hover_text(t.compare_tip).clicked() {
            self.compare_on = !self.compare_on;
        }
    }

    fn status_text(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let now = ui.input(|i| i.time);
        let running = self.running();
        let resolving = self.resolve.is_some();
        ui.add_space(4.0);
        let (icon, color) = match self.phase {
            _ if running || resolving => (ph::CIRCLE_NOTCH, BLUE),
            Phase::Done => (ph::CHECK_CIRCLE, GREEN),
            Phase::Failed => (ph::WARNING_CIRCLE, RED),
            _ => (ph::CIRCLE, STATE_OFF),
        };
        ui.label(RichText::new(icon).size(SIZE_ICON).color(color));

        let text = match self.phase {
            Phase::Running { .. } if self.waiting_comfy => t.waiting_comfy.to_string(),
            Phase::Running { .. } => format!("{}/{STAGES}  {}", self.stage + 1, t.running[self.stage]),
            _ if resolving => t.resolving.to_string(),
            Phase::Done => t.done.to_string(),
            Phase::Stopped => t.stopped.to_string(),
            Phase::Failed => t.failed.to_string(),
            Phase::Idle => t.idle.to_string(),
        };
        ui.label(if matches!(self.phase, Phase::Failed) { RichText::new(text).color(RED) } else { RichText::new(text) });

        if let (Phase::Failed, Some(failure)) = (self.phase, &self.error) {
            ui.add(egui::Label::new(RichText::new(failure_text(t, failure)).color(RED)).truncate()).on_hover_text(failure.detail());
            return;
        }
        let detail = match self.phase {
            Phase::Running { .. } if self.stage == 0 && !self.waiting_comfy && self.steps.1 == 0 => t.loading_models.to_string(),
            Phase::Running { .. } if self.stage == 0 && self.steps.1 > 0 => format!("{}/{} {}", self.steps.0, self.steps.1, t.steps),
            Phase::Done => self.entry(self.current).map(|e| format!("{}×{}", e.result.solved.layers.w, e.result.solved.layers.h)).unwrap_or_default(),
            _ => String::new(),
        };
        ui.label(RichText::new(detail).monospace().color(MUTED));
        if running {
            ui.add_space(8.0);
            let bar_w = (ui.available_width() * 0.4).clamp(80.0, 520.0);
            let (bar, _) = ui.allocate_exact_size(Vec2::new(bar_w, 8.0), Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius::same(1), LINE);
            let fill = egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * self.progress() as f32, bar.height()));
            ui.painter().rect_filled(fill, egui::CornerRadius::same(1), BLUE);
            ui.add_space(8.0);
        }
        let time = match self.phase {
            Phase::Running { started } => {
                let el = now - started;
                let left = match (self.first_step_at, self.steps) {
                    (Some(first), (v, m)) if self.stage == 0 && v >= 2 => {
                        let per = (self.last_step_at - first) / (v - 1) as f64;
                        Some((m - v) as f64 * per + AFTER_STEPS_SECONDS - (now - self.last_step_at).min(per))
                    }
                    _ => None,
                };
                match left {
                    Some(left) => format!("{} {} · {}", clock(el), t.elapsed, t.left.replace("{}", &clock(left))),
                    None => format!("{} {}", clock(el), t.elapsed),
                }
            }
            Phase::Done => self.entry(self.current).map(|e| format!("{:.1} s", e.result.seconds)).unwrap_or_default(),
            _ => String::new(),
        };
        ui.label(RichText::new(time).monospace().color(MUTED));
    }
}
