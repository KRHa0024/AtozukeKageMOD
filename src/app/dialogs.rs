//! Modal dialogs: export and the question before quitting.

use std::path::Path;

use eframe::egui::{self, Align, Id, Layout, RichText, Stroke, Vec2};
use kage::blend::{AI, ORIGINAL, SHADOW};
use kage::export::{self, Format, FORMATS};

use super::App;
use crate::theme::{AMBER, BLUE, MUTED, SEL, SIZE_TITLE};

const DIALOG_WIDTH: f32 = 420.0;

impl App {
    /// What the composite is made of, top layer first, as the layer panel shows it.
    fn composite_contents(&self) -> Option<String> {
        let t = self.t();
        let (open, close) = t.paren;
        let mut parts = Vec::new();
        if self.visible[SHADOW] {
            parts.push(format!("{}{open}{} {:.0}%{close}", t.layer_names[SHADOW], t.blends[self.cfg.blend], self.opacity[SHADOW] * 100.0));
        }
        if self.visible[AI] {
            parts.push(format!("{}{open}{:.0}%{close}", t.layer_names[AI], self.opacity[AI] * 100.0));
        }
        if self.visible[ORIGINAL] {
            parts.push(t.layer_names[ORIGINAL].to_string());
        }
        (!parts.is_empty()).then(|| parts.join(t.plus))
    }

    pub(super) fn export_modal(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let mut go = false;
        let format = FORMATS[self.cfg.export_format];
        let psd = format == Format::Psd;
        let contents = self.composite_contents();
        let ai_size = self.entry(self.current).map(|e| (e.result.shaded.w, e.result.shaded.h));
        let stem = self.entry(self.current).map(|e| Path::new(&e.result.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).unwrap_or_default();
        let resp = egui::Modal::new(Id::new("export")).show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH);
            ui.label(RichText::new(t.export).strong().size(SIZE_TITLE));
            ui.separator();
            ui.add_enabled_ui(!psd, |ui| {
                for (i, label) in t.export_targets.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.cfg.export_targets[i], *label);
                        if let (AI_TARGET, Some((w, h))) = (i, ai_size) {
                            ui.label(RichText::new(format!("{w}×{h}")).monospace().color(MUTED));
                        }
                    });
                }
            });
            if psd {
                ui.label(RichText::new(t.psd_contents).small().color(MUTED));
            } else if self.cfg.export_targets[2] {
                let warn = !self.visible[SHADOW] || !self.visible[ORIGINAL];
                let text = format!("{}: {}", t.composite_contents, contents.as_deref().unwrap_or(t.nothing_visible));
                ui.add(egui::Label::new(RichText::new(text).small().color(if warn { AMBER } else { MUTED })).wrap());
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.format).strong());
            ui.horizontal(|ui| {
                let gap = ui.spacing().item_spacing.x;
                let w = (ui.available_width() - gap * 3.0) / 4.0;
                for (i, name) in ["PNG", "JPEG", "WebP", "PSD"].iter().enumerate() {
                    if ui.add_sized([w, 28.0], egui::Button::new(*name).selected(self.cfg.export_format == i)).clicked() {
                        self.cfg.export_format = i;
                    }
                }
            });
            ui.add_space(10.0);
            let value_room = 60.0;
            ui.spacing_mut().slider_width = ui.available_width() - value_room;
            match format {
                Format::Png => {
                    ui.label(RichText::new(t.png_level).strong());
                    ui.add(egui::Slider::new(&mut self.cfg.png_level, 0..=9));
                }
                Format::Jpeg | Format::Webp => {
                    ui.label(RichText::new(if format == Format::Webp { t.quality_webp } else { t.quality }).strong());
                    ui.add(egui::Slider::new(&mut self.cfg.quality, 1..=100));
                }
                Format::Psd => {}
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.out_dir).strong());
            let row = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                if ui.button(t.browse).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_directory(&self.cfg.out_dir).pick_folder() {
                        self.cfg.out_dir = dir.display().to_string();
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.cfg.out_dir).desired_width(ui.available_width()).font(egui::TextStyle::Monospace));
            });
            let names = export::planned_names(&stem, self.cfg.export_targets, format);
            ui.add(egui::Label::new(RichText::new(names.join("  ")).small().color(MUTED)).wrap());
            ui.add_space(8.0);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can_export = psd || self.cfg.export_targets.iter().any(|&x| x);
                let b = egui::Button::new(RichText::new(t.do_export).strong().color(BLUE)).fill(SEL).stroke(Stroke::new(1.0, BLUE));
                if ui.add_enabled(can_export, b).on_disabled_hover_text(t.nothing_to_export).clicked() {
                    go = true;
                    self.export_open = false;
                }
                if ui.button(t.cancel).clicked() {
                    self.export_open = false;
                }
            });
        });
        if resp.should_close() {
            self.export_open = false;
        }
        if go {
            self.request_export(ctx);
        }
    }

    pub(super) fn quit_modal(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let resp = egui::Modal::new(Id::new("quit")).show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH - 60.0);
            ui.label(RichText::new(t.quit_title).strong().size(SIZE_TITLE));
            ui.separator();
            ui.label(t.quit_body);
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(t.quit).clicked() {
                    self.quit_confirmed = true;
                    self.quit_open = false;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button(t.cancel).clicked() {
                    self.quit_open = false;
                }
            });
        });
        if resp.should_close() {
            self.quit_open = false;
        }
    }
}

/// Row of the AI output in the export targets (it has its own size).
const AI_TARGET: usize = 1;
