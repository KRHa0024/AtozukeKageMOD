//! Top toolbar and the left panel: the image, the generation settings and Generate / Stop.

use eframe::egui::{self, Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as ph;
use kage::comfy;
use kage::pipeline::AUTO_LIGHT;

use super::App;
use crate::i18n::{Lang, T};
use crate::theme::{dashed_rect, AMBER, BLUE, CONTROL_EDGE, GREEN, MUTED, PANEL, RED, SEL, SIZE_ICON, SIZE_TITLE, TEXT};

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

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let has_result = self.current.is_some();
        ui.horizontal_centered(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  {}", ph::GEAR_SIX, t.settings)).on_hover_text(t.settings_tip).clicked() {
                    self.screen = super::Screen::Settings;
                }
                let (text, color, tip, tab) = self.readiness(t);
                let r = ui.add(egui::Label::new(RichText::new(format!("●  {text}")).color(color)).sense(Sense::click()));
                if r.on_hover_text(tip).clicked() {
                    if let Some(tab) = tab {
                        self.screen = super::Screen::Settings;
                        self.settings_tab = tab;
                    }
                }
                self.language_combo(ui);
                ui.separator();
                let export = egui::Button::new(format!("{}  {}", ph::DOWNLOAD_SIMPLE, t.export));
                if ui.add_enabled(has_result && self.export_rx.is_none(), export).on_hover_text(t.export_tip).clicked() {
                    self.export_open = true;
                }
            });
        });
    }

    /// Toolbar status: text, color, tooltip and the settings tab it leads to.
    fn readiness(&self, t: &'static T) -> (&'static str, Color32, &'static str, Option<usize>) {
        if self.verifier.running() {
            return (t.model_checking, BLUE, t.model_checking_tip, Some(0));
        }
        if !self.verifier.all_ok() {
            return (t.model_missing, AMBER, t.model_missing_tip, Some(0));
        }
        match self.comfy.state() {
            comfy::State::Ready { .. } => (t.model_ok, GREEN, t.model_ok_tip, None),
            comfy::State::Starting => (t.comfy_starting, BLUE, t.comfy_starting_tip, Some(1)),
            comfy::State::NotSet => (t.comfy_not_ready, AMBER, t.comfy_not_ready_tip, Some(1)),
            comfy::State::NotFound => (t.comfy_not_found, AMBER, t.comfy_not_ready_tip, Some(1)),
            comfy::State::MissingNodes(_) => (t.comfy_too_old, RED, t.comfy_not_ready_tip, Some(1)),
            comfy::State::Failed(_) => (t.comfy_stopped, RED, t.comfy_stopped_tip, Some(1)),
        }
    }

    pub(super) fn language_combo(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        egui::ComboBox::from_id_salt("lang")
            .width(84.0)
            .selected_text(match self.lang {
                Lang::Ja => "日本語",
                Lang::En => "English",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.lang, Lang::Ja, "日本語");
                ui.selectable_value(&mut self.lang, Lang::En, "English");
            })
            .response
            .on_hover_text(t.language_tip);
        ui.label(RichText::new(ph::GLOBE).size(SIZE_ICON)).on_hover_text(t.language_tip);
    }

    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let running = self.running();
        let ready = self.ready();

        egui::Panel::bottom("generate").frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(0, 10))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width() - STOP_WIDTH - ui.spacing().item_spacing.x;
                let gen = egui::Button::new(RichText::new(format!("{}  {}", ph::PLAY, t.generate)).size(SIZE_TITLE).strong().color(BLUE))
                    .fill(SEL)
                    .stroke(Stroke::new(1.0, BLUE));
                let enabled = self.image.is_some() && !running && ready;
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
            // Open / drop box
            let inner = Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let open = egui::Button::new(RichText::new(format!("{}  {}", ph::FOLDER_OPEN, t.open_image)).strong());
                if ui.add_sized([ui.available_width(), ACTION_HEIGHT - 4.0], open).on_hover_text(t.open_tip).clicked() {
                    self.pick_image(&ctx);
                }
                ui.vertical_centered(|ui| ui.label(RichText::new(t.drop_hint).small().color(MUTED)));
                if let Some(img) = &self.image {
                    ui.separator();
                    let row = Vec2::new(ui.available_width(), 20.0);
                    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}×{}", img.img.w, img.img.h)).monospace().color(MUTED));
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(&img.name).truncate()).on_hover_text(&img.name);
                        });
                    });
                }
                if let Some(err) = &self.load_error {
                    ui.label(RichText::new(err).small().color(RED));
                }
            });
            dashed_rect(ui.painter(), inner.response.rect);

            ui.add_space(12.0);
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

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let label = ui.add(egui::Label::new(RichText::new(t.strength).strong()).sense(Sense::click()));
                if label.on_hover_text(format!("{}\n{}", t.strength_tip, t.reset_tip)).double_clicked() {
                    self.cfg.strength = STRENGTH_DEFAULT;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(egui::DragValue::new(&mut self.cfg.strength).range(0.5..=1.5).speed(0.01).fixed_decimals(2).prefix("× "));
                });
            });
            ui.spacing_mut().slider_width = ui.available_width();
            let slider = ui.add(egui::Slider::new(&mut self.cfg.strength, 0.5..=1.5).step_by(0.05).show_value(false));
            // The neutral value.
            let x = slider.rect.center().x;
            ui.painter().vline(x, slider.rect.y_range(), Stroke::new(1.0, CONTROL_EDGE));

            ui.add_space(10.0);
            ui.checkbox(&mut self.cfg.bg_shadow, t.bg_shadow).on_hover_text(t.bg_tip);
            ui.checkbox(&mut self.cfg.gray, t.gray).on_hover_text(t.gray_tip);

            ui.add_space(8.0);
            ui.separator();
            let changed = [!self.cfg.extra.trim().is_empty(), self.cfg.ghost, self.seed.trim() != "random"].iter().filter(|&&on| on).count();
            let title = if changed > 0 { format!("{}{}{changed}{}", t.advanced, t.paren.0, t.paren.1) } else { t.advanced.to_string() };
            egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt("advanced").default_open(self.adv_default_open).show(ui, |ui| {
                ui.label(t.extra);
                ui.add(egui::TextEdit::singleline(&mut self.cfg.extra).hint_text("warm sunset light").desired_width(f32::INFINITY));
                ui.label(RichText::new(t.extra_desc).small().color(MUTED));
                ui.add_space(8.0);
                ui.checkbox(&mut self.cfg.ghost, t.ghost);
                ui.label(RichText::new(t.ghost_desc).small().color(MUTED));
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
                ui.add_space(8.0);
                let b = egui::Button::new(format!("{}  {}", ph::SELECTION_FOREGROUND, t.extract));
                let can_extract = self.image.is_some() && !running && (self.cfg.bg_shadow || ready);
                let resp = ui.add_enabled(can_extract, b.min_size(Vec2::new(ui.available_width(), 28.0)));
                if resp.on_disabled_hover_text(t.extract_needs_comfy).clicked() {
                    self.pick_shaded(&ctx);
                }
                ui.label(RichText::new(t.extract_desc).small().color(MUTED));
            });
        });
    }
}
