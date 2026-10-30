//! Top toolbar and the left panel: the settings that need a new AI run, and Generate / Stop.

use eframe::egui::{self, Align, Color32, Frame, Layout, Margin, RichText, Sense, Vec2};
use egui_phosphor::regular as ph;
use kage::comfy;
use kage::models::{Status, MODELS};
use kage::pipeline::AUTO_LIGHT;

use super::{App, Screen, MODELS_TAB};
use crate::i18n::{Lang, T};
use crate::theme::{primary_button, AMBER, BLUE, GREEN, MUTED, PANEL, RED, SIZE_BODY, SIZE_ICON, SIZE_TITLE, TEXT};

const DIR_ICONS: [&str; 9] = [
    ph::ARROW_DOWN_RIGHT,
    ph::ARROW_DOWN,
    ph::ARROW_DOWN_LEFT,
    ph::ARROW_RIGHT,
    "",
    ph::ARROW_LEFT,
    ph::ARROW_UP_RIGHT,
    ph::ARROW_UP,
    ph::ARROW_UP_LEFT,
];

const LIGHT_CELL_HEIGHT: f32 = 28.0;
const STOP_WIDTH: f32 = 84.0;
const ACTION_HEIGHT: f32 = 48.0;
/// Severity of the toolbar status, mild to serious.
const SEVERITY: [Color32; 3] = [BLUE, AMBER, RED];

struct Part {
    text: String,
    severity: usize,
    tip: &'static str,
}

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let has_result = self.current.is_some();
        ui.horizontal_centered(|ui| {
            if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_image)).on_hover_text(t.open_tip).clicked() {
                self.pick_image(&ctx);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  {}", ph::GEAR_SIX, t.settings)).on_hover_text(t.settings_tip).clicked() {
                    self.screen = Screen::Settings;
                }
                let can_export = has_result && self.export_rx.is_none();
                let export = primary_button(format!("{}  {}", ph::DOWNLOAD_SIMPLE, t.export), SIZE_BODY, has_result);
                if ui.add_enabled(can_export, export).on_hover_text(t.export_tip).clicked() {
                    self.export_open = true;
                }
                let (text, color, tip, tab) = self.readiness(t);
                let r = ui.add(egui::Label::new(RichText::new(format!("●  {text}")).color(color)).sense(Sense::click()));
                if r.on_hover_text(tip).clicked() {
                    if let Some(tab) = tab {
                        self.screen = Screen::Settings;
                        self.settings_tab = tab;
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if let Some(img) = &self.image {
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(&img.name).truncate()).on_hover_text(&img.name);
                        ui.label(RichText::new(format!("{}×{}", img.img.w, img.img.h)).monospace().color(MUTED));
                    }
                });
            });
        });
    }

    /// The settings tab that still needs something from the user.
    pub(super) fn setup_tab(&self) -> Option<usize> {
        if self.comfy_part(self.t()).is_some_and(|p| p.severity > 0) {
            return Some(super::COMFY_TAB);
        }
        self.model_part(self.t()).filter(|p| p.severity > 0).map(|_| MODELS_TAB)
    }

    fn comfy_part(&self, t: &'static T) -> Option<Part> {
        let (text, severity, tip) = match self.comfy.state() {
            comfy::State::Ready { .. } => return None,
            comfy::State::Starting => (t.comfy_starting, 0, t.comfy_starting_tip),
            comfy::State::NotSet => (t.comfy_not_ready, 1, t.comfy_not_ready_tip),
            comfy::State::NotFound => (t.comfy_not_found, 1, t.comfy_not_ready_tip),
            comfy::State::MissingNodes(_) => (t.comfy_too_old, 2, t.comfy_not_ready_tip),
            comfy::State::Failed(_) => (t.comfy_stopped, 2, t.comfy_stopped_tip),
        };
        Some(Part { text: text.to_string(), severity, tip })
    }

    fn model_part(&self, t: &'static T) -> Option<Part> {
        if self.verifier.running() {
            return Some(Part { text: t.model_checking.to_string(), severity: 0, tip: t.model_checking_tip });
        }
        if self.verifier.all_ok() {
            return None;
        }
        let ok = self.verifier.statuses().iter().filter(|s| **s == Status::Ok).count();
        let text = t.model_count.replacen("{}", &ok.to_string(), 1).replacen("{}", &MODELS.len().to_string(), 1);
        Some(Part { text, severity: 1, tip: t.model_missing_tip })
    }

    /// Toolbar status: everything still missing at once, its color, tooltip and the settings tab it leads to.
    fn readiness(&self, t: &'static T) -> (String, Color32, &'static str, Option<usize>) {
        let parts: Vec<Part> = [self.comfy_part(t), self.model_part(t)].into_iter().flatten().collect();
        let Some(first) = parts.first() else { return (t.model_ok.to_string(), GREEN, t.model_ok_tip, None) };
        let text = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ・ ");
        let severity = parts.iter().map(|p| p.severity).max().unwrap_or(0);
        (text, SEVERITY[severity], first.tip, self.setup_tab())
    }

    pub(super) fn language_combo(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_id_salt("lang")
            .width(100.0)
            .selected_text(match self.lang {
                Lang::Ja => "日本語",
                Lang::En => "English",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.lang, Lang::Ja, "日本語");
                ui.selectable_value(&mut self.lang, Lang::En, "English");
            });
    }

    /// Settings that differ from what the shown result was generated with.
    fn unapplied(&self, t: &'static T) -> Vec<&'static str> {
        let mut items = Vec::new();
        let Some(e) = self.entry(self.current) else { return items };
        let p = &e.result.params;
        if !e.result.extracted {
            if self.cfg.light_dir != p.light_dir {
                items.push(t.unapplied_light);
            }
            if self.cfg.extra.trim() != p.extra.trim() {
                items.push(t.unapplied_extra);
            }
            if self.seed.trim().parse::<u64>().is_ok_and(|seed| seed != p.seed) {
                items.push(t.unapplied_seed);
            }
        }
        // Only here when the subject mask cannot be made right now.
        if !self.cfg.bg_shadow && p.background && e.result.mask.is_none() && self.mask_run.is_none() {
            items.push(t.unapplied_background);
        }
        items
    }

    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let running = self.running();
        let ready = self.ready();

        egui::Panel::bottom("generate").frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(0, 10))).show(ui, |ui| {
            let unapplied = self.unapplied(t);
            if !unapplied.is_empty() {
                let text = t.unapplied.replace("{}", &unapplied.join(t.list_sep));
                ui.add(egui::Label::new(RichText::new(text).color(AMBER)).wrap());
            }
            let extract = egui::Button::new(format!("{}  {}", ph::SELECTION_FOREGROUND, t.extract));
            let can_extract = self.image.is_some() && !running && (self.cfg.bg_shadow || ready);
            let resp = ui.add_enabled(can_extract, extract.min_size(Vec2::new(ui.available_width(), 28.0)));
            let resp = resp.on_hover_text(t.extract_desc).on_disabled_hover_text(if ready { t.extract_desc } else { t.extract_needs_comfy });
            if resp.clicked() {
                self.pick_shaded(&ctx);
            }
            ui.horizontal(|ui| {
                let w = ui.available_width() - STOP_WIDTH - ui.spacing().item_spacing.x;
                let enabled = self.image.is_some() && !running && ready;
                let gen = primary_button(format!("{}  {}", ph::PLAY, t.generate), SIZE_TITLE, enabled);
                let resp = ui.add_enabled_ui(enabled, |ui| ui.add_sized([w, ACTION_HEIGHT], gen)).inner;
                let resp = if !ready { resp.on_disabled_hover_text(t.not_ready_tip) } else { resp.on_hover_text(t.generate_tip) };
                if resp.clicked() {
                    self.start_run(&ctx, None);
                }
                let stop = egui::Button::new(format!("{}  {}", ph::STOP, t.stop));
                if ui.add_enabled_ui(running, |ui| ui.add_sized([STOP_WIDTH, ACTION_HEIGHT], stop)).inner.on_hover_text(t.stop_tip).clicked() {
                    self.stop_run();
                }
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t.light_dir).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(t.light_short[self.cfg.light_dir]).color(MUTED)));
            });
            let w = (ui.available_width() - 6.0) / 3.0;
            egui::Grid::new("dirs").spacing([3.0, 3.0]).show(ui, |ui| {
                for (i, icon) in DIR_ICONS.iter().enumerate() {
                    let text = if i == AUTO_LIGHT { RichText::new(t.light_short[i]) } else { RichText::new(*icon).size(SIZE_ICON) };
                    let b = egui::Button::new(text).selected(self.cfg.light_dir == i);
                    if ui.add_sized([w, LIGHT_CELL_HEIGHT], b).on_hover_text(t.light_tips[i]).clicked() {
                        self.cfg.light_dir = i;
                    }
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });

            ui.add_space(8.0);
            ui.separator();
            let changed = [!self.cfg.extra.trim().is_empty(), self.seed.trim() != "random"].iter().filter(|&&on| on).count();
            let title = if changed > 0 { format!("{}{}{changed}{}", t.advanced, t.paren.0, t.paren.1) } else { t.advanced.to_string() };
            egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt("advanced").default_open(self.adv_default_open).show(ui, |ui| {
                ui.label(t.extra);
                ui.add(egui::TextEdit::singleline(&mut self.cfg.extra).hint_text("warm sunset light").desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.label(t.seed);
                let seed_ok = self.seed.trim() == "random" || self.seed.trim().parse::<u64>().is_ok();
                ui.horizontal(|ui| {
                    let edit = egui::TextEdit::singleline(&mut self.seed).desired_width(ui.available_width() - 34.0).text_color(if seed_ok { TEXT } else { RED });
                    ui.add(edit);
                    if ui.button(ph::ARROWS_CLOCKWISE).on_hover_text(t.seed_random).clicked() {
                        self.seed = "random".into();
                    }
                });
                if !seed_ok {
                    ui.label(RichText::new(t.seed_invalid).small().color(RED));
                }
            });
        });
    }
}
