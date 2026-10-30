//! Settings screen: models, ComfyUI, log.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Stroke};
use egui_phosphor::regular as ph;
use kage::comfy;
use kage::log::Level;
use kage::models::{self, Status, MODELS};

use super::{App, Screen, COMFY_TAB, MODELS_TAB};
use crate::i18n::T;
use crate::theme::{AMBER, BLUE, GREEN, LINE, MUTED, RED, SIZE_ICON, SIZE_TITLE, TEXT};

pub(super) const COMFY_RELEASES: &str = "https://github.com/comfyanonymous/ComfyUI/releases";
/// Order of the tabs in the list: set up ComfyUI first, as the README says.
const NAV_ORDER: [usize; 3] = [COMFY_TAB, MODELS_TAB, 2];
const NAV_DOT_RADIUS: f32 = 4.0;
/// Long explanations stay readable on wide windows.
const TEXT_WIDTH: f32 = 640.0;
const CONTENT_MIN_WIDTH: f32 = 720.0;
const SIZE_COLUMN: f32 = 70.0;
const STATE_COLUMN: f32 = 170.0;
const ACTIONS_COLUMN: f32 = 240.0;
const HEADER_HEIGHT: f32 = 20.0;
const ROW_HEIGHT: f32 = 36.0;
const MIN_FILE_COLUMN: f32 = 120.0;
const ROW_MARGIN: i8 = 8;

/// One line of the model list: file (takes what is left), size, state, actions.
fn model_columns(
    ui: &mut egui::Ui,
    height: f32,
    file: impl FnOnce(&mut egui::Ui),
    size: impl FnOnce(&mut egui::Ui),
    state: impl FnOnce(&mut egui::Ui),
    actions: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        let gaps = ui.spacing().item_spacing.x * 3.0;
        let rest = (ui.available_width() - SIZE_COLUMN - STATE_COLUMN - ACTIONS_COLUMN - gaps).max(MIN_FILE_COLUMN);
        for (width, cell) in [
            (rest, Box::new(file) as Box<dyn FnOnce(&mut egui::Ui) + '_>),
            (SIZE_COLUMN, Box::new(size)),
            (STATE_COLUMN, Box::new(state)),
            (ACTIONS_COLUMN, Box::new(actions)),
        ] {
            ui.allocate_ui_with_layout(egui::vec2(width, height), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(width);
                ui.set_height(height);
                cell(ui);
            });
        }
    });
}

/// A directory or file in Explorer, or a URL in the browser.
pub(super) fn open_in_explorer(target: &Path) {
    let text = target.to_string_lossy();
    // Explorer takes only backslashes in local paths.
    let target = if text.contains("://") { text.into_owned() } else { text.replace('/', "\\") };
    let _ = std::process::Command::new("explorer").arg(target).spawn();
}

impl App {
    pub(super) fn settings_toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal_centered(|ui| {
            if ui.button(format!("{}  {}", ph::ARROW_LEFT, t.back)).on_hover_text(t.back_tip).clicked() {
                self.apply_dirs();
                self.screen = Screen::Main;
            }
            ui.label(RichText::new(t.settings).strong());
        });
    }

    /// State of a settings tab for its dot in the list; the log has none.
    fn tab_state(&self, tab: usize) -> Option<egui::Color32> {
        match tab {
            COMFY_TAB => Some(match self.comfy.state() {
                comfy::State::Ready { .. } => GREEN,
                comfy::State::Starting => BLUE,
                comfy::State::MissingNodes(_) | comfy::State::Failed(_) => RED,
                _ => AMBER,
            }),
            MODELS_TAB if self.verifier.running() => Some(BLUE),
            MODELS_TAB => Some(if self.verifier.all_ok() { GREEN } else { AMBER }),
            _ => None,
        }
    }

    pub(super) fn settings_nav(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        for tab in NAV_ORDER {
            let resp = ui.add_sized([ui.available_width(), 30.0], egui::Button::new(t.tabs[tab]).selected(self.settings_tab == tab));
            if let Some(color) = self.tab_state(tab) {
                ui.painter().circle_filled(egui::pos2(resp.rect.right() - 14.0, resp.rect.center().y), NAV_DOT_RADIUS, color);
            }
            if resp.clicked() {
                self.settings_tab = tab;
            }
        }
    }

    pub(super) fn settings_body(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        // Text wraps at the window width, down to a width the tables need; narrower windows scroll.
        let width = ui.available_width().max(CONTENT_MIN_WIDTH);
        match self.settings_tab {
            MODELS_TAB => {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(width);
                    self.models_tab(ui, t)
                });
            }
            COMFY_TAB => {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(width);
                    self.comfy_tab(ui, t)
                });
            }
            _ => self.log_tab(ui, t),
        }
    }

    fn models_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let statuses = self.verifier.statuses();
        let notes = self.verifier.notes();
        let checking = self.verifier.running();
        let ok = statuses.iter().filter(|s| **s == Status::Ok).count();
        if checking {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.models).strong().size(SIZE_TITLE));
            ui.label(RichText::new(format!("{ok}/{} {}", MODELS.len(), t.match_word)).color(if ok == MODELS.len() { GREEN } else { AMBER }));
            ui.label(RichText::new(t.total.replace("{}", &format!("{:.1}", models::total_bytes() as f64 / 1e9))).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(format!("{}  {}", ph::ARROWS_CLOCKWISE, t.recheck));
                if ui.add_enabled(!checking, b).on_hover_text(t.recheck_tip).clicked() {
                    self.verifier.start(PathBuf::from(self.cfg.model_dir.trim()));
                    // A file that just became valid must be picked up by ComfyUI's model list.
                    if !matches!(self.comfy.state(), comfy::State::Ready { .. } | comfy::State::Starting) && !self.running() {
                        self.comfy.start(&self.cfg.comfy_dir, Path::new(self.cfg.model_dir.trim()));
                    }
                }
            });
        });
        ui.add(egui::Label::new(t.models_note).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(t.model_folder);
            let r = ui.add(egui::TextEdit::singleline(&mut self.cfg.model_dir).desired_width(ui.available_width() - 62.0).font(egui::TextStyle::Monospace));
            if r.lost_focus() {
                self.apply_dirs();
            }
            if ui.button(t.open).on_hover_text(t.open_folder_tip).clicked() {
                let dir = PathBuf::from(self.cfg.model_dir.trim());
                let _ = models::prepare_dir(&dir);
                open_in_explorer(&dir);
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new(t.layout_title).strong());
        self.model_layout(ui);
        ui.add_space(12.0);
        ui.label(RichText::new(t.pick_title).strong());
        ui.add_space(2.0);
        let header = Frame::new().inner_margin(Margin::symmetric(ROW_MARGIN, 0));
        header.show(ui, |ui| {
            model_columns(
            ui,
            HEADER_HEIGHT,
            |ui| {
                ui.label(RichText::new(t.col_file).color(MUTED));
            },
            |ui| {
                ui.label(RichText::new(t.col_size).color(MUTED));
            },
            |ui| {
                ui.label(RichText::new(t.col_hash).color(MUTED));
            },
            |ui| {
                ui.label(RichText::new(t.col_source).color(MUTED));
            },
            );
        });
        for (i, m) in MODELS.iter().enumerate() {
            let (icon, color, text) = match &statuses[i] {
                Status::Ok => (ph::CHECK_CIRCLE, GREEN, t.states[0].to_string()),
                Status::Checking(f) => (ph::CIRCLE_NOTCH, BLUE, t.states[1].replace("{}", &format!("{:.0}", f * 100.0))),
                Status::Missing => (ph::CIRCLE_DASHED, AMBER, t.states[2].to_string()),
                Status::Mismatch => (ph::X_CIRCLE, RED, t.states[3].to_string()),
                Status::Unchecked => (ph::CIRCLE, MUTED, t.states[4].to_string()),
                Status::Copying(f) => (ph::CIRCLE_NOTCH, BLUE, t.states[5].replace("{}", &format!("{:.0}", f * 100.0))),
            };
            let note = match &notes[i] {
                Some(models::Note::PickedMismatch) => Some(t.picked_mismatch.to_string()),
                Some(models::Note::PickedFailed(e)) => Some(t.picked_failed.replace("{}", e)),
                None => None,
            };
            Frame::new().fill(egui::Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::symmetric(ROW_MARGIN, 5)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                model_columns(
                    ui,
                    ROW_HEIGHT,
                    |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add(egui::Label::new(RichText::new(m.file).monospace()).truncate()).on_hover_text(m.file);
                            ui.label(RichText::new(t.purposes[i]).small().color(MUTED));
                        });
                    },
                    |ui| {
                        ui.label(RichText::new(format!("{:.2} GB", m.size as f64 / 1e9)).monospace());
                    },
                    |ui| {
                        ui.label(RichText::new(icon).size(SIZE_ICON).color(color));
                        ui.label(RichText::new(text).color(color)).on_hover_text(m.sha256);
                    },
                    |ui| {
                        if ui.button(format!("{}  {}", ph::LINK, t.source_page)).on_hover_text(m.source).clicked() {
                            open_in_explorer(Path::new(m.source));
                        }
                        let pick = egui::Button::new(format!("{}  {}", ph::FILE_ARROW_UP, t.pick_file));
                        if ui.add_enabled(!checking, pick).on_hover_text(t.pick_file_tip).clicked() {
                            let mut dialog = rfd::FileDialog::new().set_title(format!("{} — {}", t.pick_file, m.file)).add_filter("safetensors", &["safetensors"]);
                            if let Some(downloads) = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")) {
                                dialog = dialog.set_directory(downloads);
                            }
                            if let Some(src) = dialog.pick_file() {
                                self.verifier.place(i, src, PathBuf::from(self.cfg.model_dir.trim()));
                            }
                        }
                    },
                );
                if let Some(note) = note {
                    ui.add(egui::Label::new(RichText::new(note).small().color(RED)).wrap());
                }
            });
        }
        ui.add_space(2.0);
        ui.label(RichText::new(t.pick_note).color(MUTED));
    }

    /// The directory tree the models have to be in.
    fn model_layout(&self, ui: &mut egui::Ui) {
        let dir = Path::new(self.cfg.model_dir.trim());
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "models".into());
        Frame::new().fill(egui::Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(format!("{name}/")).monospace());
            for (i, m) in MODELS.iter().enumerate() {
                let branch = if i + 1 == MODELS.len() { "└─" } else { "├─" };
                ui.label(RichText::new(format!("{branch} {}/{}", m.folder, m.file)).monospace());
            }
        });
    }

    fn comfy_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let state = self.comfy.state();
        let running = self.running();
        if state == comfy::State::Starting {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        ui.label(RichText::new("ComfyUI").strong().size(SIZE_TITLE));
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.comfy_install).color(MUTED));
            if ui.button(format!("{}  {}", ph::LINK, t.source_page)).on_hover_text(COMFY_RELEASES).clicked() {
                open_in_explorer(Path::new(COMFY_RELEASES));
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("comfy").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label(t.location);
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.cfg.comfy_dir).desired_width(520.0).font(egui::TextStyle::Monospace));
                if r.lost_focus() {
                    self.apply_dirs();
                }
                if ui.button(t.browse).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.cfg.comfy_dir = dir.display().to_string();
                        self.apply_dirs();
                    }
                }
            });
            ui.end_row();
            ui.label(t.state);
            ui.horizontal(|ui| {
                let (color, text) = match &state {
                    comfy::State::NotSet => (MUTED, t.comfy_states[0].to_string()),
                    comfy::State::NotFound => (AMBER, t.comfy_states[1].to_string()),
                    comfy::State::Starting => (BLUE, t.comfy_states[2].to_string()),
                    comfy::State::Ready { version } => (GREEN, t.comfy_states[3].replace("{}", version)),
                    comfy::State::MissingNodes(_) => (RED, t.comfy_states[4].to_string()),
                    comfy::State::Failed(_) => (RED, t.comfy_states[5].to_string()),
                };
                ui.label(RichText::new("●").color(color));
                ui.label(RichText::new(text).color(color));
                if !self.cfg.comfy_dir.trim().is_empty() {
                    let restart = egui::Button::new(format!("{}  {}", ph::ARROWS_CLOCKWISE, t.restart));
                    if ui.add_enabled(!running, restart).on_hover_text(t.restart_tip).on_disabled_hover_text(t.restart_tip).clicked() {
                        self.comfy.start(&self.cfg.comfy_dir, Path::new(self.cfg.model_dir.trim()));
                        self.applied_dirs = (self.cfg.model_dir.trim().to_string(), self.cfg.comfy_dir.trim().to_string());
                    }
                }
                if self.comfy.log_path().is_file() && ui.button(format!("{}  {}", ph::FILE_TEXT, t.comfy_log)).clicked() {
                    open_in_explorer(&self.comfy.log_path());
                }
            });
            ui.end_row();
            if let comfy::State::Failed(e) = &state {
                ui.label("");
                ui.add(egui::Label::new(RichText::new(e).monospace().small().color(RED)).wrap());
                ui.end_row();
            }
        });
        if let Some(dir) = self.comfy_models_to_use() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t.models_found);
                if ui.button(t.use_models).on_hover_text(dir.display().to_string()).clicked() {
                    self.cfg.model_dir = dir.display().to_string();
                    self.apply_dirs();
                }
            });
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(t.gpu_memory).strong());
        ui.checkbox(&mut self.cfg.keep_loaded, t.keep_loaded);
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.set_max_width(TEXT_WIDTH);
            ui.add(egui::Label::new(RichText::new(t.keep_loaded_desc).color(MUTED)).wrap());
        });
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(t.display_section).strong());
        ui.horizontal(|ui| {
            ui.label(t.display_language);
            self.language_combo(ui);
        });
    }

    /// The chosen ComfyUI's own models directory, when it holds all the files and is not in use yet.
    pub(super) fn comfy_models_to_use(&self) -> Option<PathBuf> {
        let install = comfy::detect(Path::new(self.cfg.comfy_dir.trim()))?;
        let dir = install.main_py.parent()?.join("models");
        (dir != Path::new(self.cfg.model_dir.trim()) && models::all_present(&dir)).then_some(dir)
    }

    fn log_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let (serial, lines) = self.log.snapshot();
        let filter = [None, Some(Level::Info), Some(Level::Warn), Some(Level::Error)][self.log_level];
        let shown: Vec<_> = lines.iter().filter(|l| filter.is_none_or(|f| f == l.level)).collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.log).strong().size(SIZE_TITLE));
            ui.add_space(8.0);
            for (i, name) in t.levels.iter().enumerate() {
                if ui.add(egui::Button::new(*name).selected(self.log_level == i)).clicked() {
                    self.log_level = i;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(t.open_log_folder).clicked() {
                    open_in_explorer(self.log.dir());
                }
                if ui.button(t.copy).on_hover_text(t.copy_tip).clicked() {
                    let text = shown.iter().map(|l| format!("{}  {:<5}  {}", l.time, l.level.tag(), l.text)).collect::<Vec<_>>().join("\n");
                    ui.ctx().copy_text(text);
                }
            });
        });
        ui.add_space(6.0);
        let stick = serial != self.log_seen;
        self.log_seen = serial;
        Frame::new().fill(egui::Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::both().auto_shrink([false, false]).stick_to_bottom(stick).show(ui, |ui| {
                for l in &shown {
                    let color = match l.level {
                        Level::Warn => AMBER,
                        Level::Error => RED,
                        Level::Info => TEXT,
                    };
                    ui.add(egui::Label::new(RichText::new(format!("{}  {:<5}  {}", l.time, l.level.tag(), l.text)).monospace().color(color)).extend());
                }
            });
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}
