//! Top bar and the left panel: the work starts here and runs down the panel to Generate.

use eframe::egui::{self, Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as ph;
use kage::comfy;
use kage::pipeline::AUTO_LIGHT;

use super::status::clock;
use super::{App, Phase, Screen, COMFY_TAB, MODELS_TAB};
use crate::i18n::{Lang, T};
use crate::theme::{primary_button, AMBER, BLUE, CONTROL_EDGE, GREEN, LINE, MUTED, PANEL, RED, SIZE_ICON, SIZE_TITLE, TEXT};

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
const STRENGTH_DEFAULT: f32 = 1.0;
const STOP_WIDTH: f32 = 84.0;
const ACTION_HEIGHT: f32 = 48.0;
/// Height of the guide area above Generate, so the buttons do not move with its text.
const GUIDE_HEIGHT: f32 = 44.0;
/// Sources above this take the longer time in the guide (the model runs at its higher resolution).
const LARGE_SOURCE_PIXELS: usize = 2_500_000;
/// Width of the strength value field.
const VALUE_WIDTH: f32 = 66.0;

/// One thing that is not ready yet.
struct Part {
    text: &'static str,
    /// 0 in progress, 1 to set up, 2 broken.
    severity: usize,
    tip: &'static str,
}

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        ui.horizontal_centered(|ui| {
            if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_image)).on_hover_text(t.open_tip).clicked() {
                self.pick_image(&ctx);
            }
            let extract = egui::Button::new(format!("{}  {}", ph::SELECTION_FOREGROUND, t.extract));
            let can_extract = self.image.is_some() && !self.running() && (self.cfg.bg_shadow || self.ready());
            let resp = ui.add_enabled(can_extract, extract).on_hover_text(t.extract_desc);
            let reason = if self.image.is_none() || self.ready() { t.extract_desc } else { t.extract_needs_comfy };
            if resp.on_disabled_hover_text(reason).clicked() {
                self.pick_shaded(&ctx);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(ph::GEAR_SIX).on_hover_text(t.settings).clicked() {
                    self.screen = Screen::Settings;
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
            return Some(COMFY_TAB);
        }
        self.model_part(self.t()).filter(|p| p.severity > 0).map(|_| MODELS_TAB)
    }

    fn comfy_part(&self, t: &'static T) -> Option<Part> {
        let (text, severity, tip) = match self.comfy.state() {
            comfy::State::Ready { .. } => return None,
            comfy::State::Starting => (t.comfy_starting, 0, t.comfy_starting_tip),
            comfy::State::NotSet => (t.not_set_up, 1, t.comfy_not_ready_tip),
            comfy::State::NotFound => (t.comfy_not_found, 1, t.comfy_not_ready_tip),
            comfy::State::MissingNodes(_) => (t.comfy_too_old, 2, t.comfy_not_ready_tip),
            comfy::State::Failed(_) => (t.comfy_stopped, 2, t.comfy_stopped_tip),
        };
        Some(Part { text, severity, tip })
    }

    fn model_part(&self, t: &'static T) -> Option<Part> {
        if self.verifier.running() {
            return Some(Part { text: t.model_checking, severity: 0, tip: t.model_checking_tip });
        }
        if self.verifier.all_ok() {
            return None;
        }
        Some(Part { text: t.not_set_up, severity: 1, tip: t.model_missing_tip })
    }

    /// Top bar status: the most serious thing still missing, its color, tooltip and the settings tab it leads to.
    fn readiness(&self, t: &'static T) -> (&'static str, Color32, &'static str, Option<usize>) {
        let parts = [self.comfy_part(t), self.model_part(t)];
        let Some(worst) = parts.iter().flatten().max_by_key(|p| p.severity) else { return (t.model_ok, GREEN, t.model_ok_tip, None) };
        let color = [BLUE, AMBER, RED][worst.severity];
        (worst.text, color, worst.tip, self.setup_tab())
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
    pub(super) fn unapplied(&self, t: &'static T) -> Vec<&'static str> {
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

    /// One place above Generate says what happens next, without hovering over anything.
    fn guide(&self, ui: &mut egui::Ui, t: &'static T, unapplied: &[&'static str]) {
        let now = ui.input(|i| i.time);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), GUIDE_HEIGHT), Sense::hover());
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::bottom_up(Align::Min)));
        let ui = &mut ui;
        if let Phase::Running { .. } = self.phase {
            let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius::same(1), LINE);
            let fill = egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * self.progress() as f32, bar.height()));
            ui.painter().rect_filled(fill, egui::CornerRadius::same(1), BLUE);
            if self.steps.1 > 0 {
                let mut text = format!("{}/{} {}", self.steps.0, self.steps.1, t.steps);
                if let Some(left) = self.remaining(now) {
                    text = format!("{text}{}{}", t.list_sep, t.left.replace("{}", &clock(left)));
                }
                ui.label(RichText::new(text).color(TEXT));
            }
            return;
        }
        let muted = |ui: &mut egui::Ui, text: &str| {
            ui.add(egui::Label::new(RichText::new(text).color(MUTED)).wrap());
        };
        if !unapplied.is_empty() {
            let text = t.unapplied.replace("{}", &unapplied.join(t.list_sep));
            ui.add(egui::Label::new(RichText::new(text).color(AMBER)).wrap());
        } else if !self.ready() {
            muted(ui, t.guide_setup);
        } else if let Some(img) = &self.image {
            if self.current.is_none() {
                let large = img.img.w * img.img.h > LARGE_SOURCE_PIXELS;
                muted(ui, t.guide_first_run);
                muted(ui, &t.guide_time.replace("{}", if large { t.guide_time_large } else { t.guide_time_normal }));
            }
        } else {
            muted(ui, t.guide_open);
        }
    }

    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let running = self.running();
        let ready = self.ready();
        let unapplied = self.unapplied(t);

        egui::Panel::bottom("generate").frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(0, 10))).show(ui, |ui| {
            self.guide(ui, t, &unapplied);
            ui.horizontal(|ui| {
                let w = ui.available_width() - STOP_WIDTH - ui.spacing().item_spacing.x;
                let enabled = self.image.is_some() && !running && ready;
                let main_action = enabled && (self.current.is_none() || !unapplied.is_empty());
                let gen = primary_button(format!("{}  {}", ph::PLAY, t.generate), SIZE_TITLE, main_action);
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
            // Before the setup is done, the guide on the canvas is the only thing to look at.
            ui.add_enabled_ui(self.image.is_some() || self.setup_tab().is_none(), |ui| self.shadow_settings(ui, t));
        });
    }

    /// Everything that decides the shadow, in the order it is decided.
    fn shadow_settings(&mut self, ui: &mut egui::Ui, t: &'static T) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.light_dir).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(t.light_short[self.cfg.light_dir]).color(MUTED)));
        });
        let w = (ui.available_width() - 6.0) / 3.0;
        egui::Grid::new("dirs").spacing([3.0, 3.0]).show(ui, |ui| {
            for (i, icon) in DIR_ICONS.iter().enumerate() {
                let text = if i == AUTO_LIGHT { RichText::new(t.light_auto) } else { RichText::new(*icon).size(SIZE_ICON) };
                let b = egui::Button::new(text).selected(self.cfg.light_dir == i);
                if ui.add_sized([w, LIGHT_CELL_HEIGHT], b).on_hover_text(t.light_tips[i]).clicked() {
                    self.cfg.light_dir = i;
                }
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = ui.add(egui::Label::new(RichText::new(t.strength).strong()).sense(Sense::click()));
            if label.on_hover_text(format!("{}\n{}", t.strength_tip, t.reset_tip)).double_clicked() {
                self.cfg.strength = STRENGTH_DEFAULT;
            }
            ui.spacing_mut().slider_width = ui.available_width() - VALUE_WIDTH - ui.spacing().item_spacing.x;
            let slider = ui.add(egui::Slider::new(&mut self.cfg.strength, 0.5..=1.5).step_by(0.05).show_value(false));
            // The neutral value.
            ui.painter().vline(slider.rect.center().x, slider.rect.y_range(), Stroke::new(1.0, CONTROL_EDGE));
            ui.add_sized([VALUE_WIDTH, 22.0], egui::DragValue::new(&mut self.cfg.strength).range(0.5..=1.5).speed(0.01).fixed_decimals(2).prefix("× "));
        });
        ui.add_space(6.0);
        ui.checkbox(&mut self.cfg.bg_shadow, t.bg_shadow).on_hover_text(t.bg_tip);
        ui.checkbox(&mut self.cfg.gray, t.gray).on_hover_text(t.gray_tip);
        ui.checkbox(&mut self.cfg.ghost, t.ghost).on_hover_text(t.ghost_desc);

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
    }
}
