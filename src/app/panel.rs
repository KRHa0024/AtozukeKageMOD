//! Right panel: the layer list and the history of runs.

use eframe::egui::{self, Align2, Color32, FontId, Frame, Id, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, TextureId, Vec2};
use egui_phosphor::regular as ph;
use kage::blend::{AI, MENU_ORDER, ORIGINAL, SHADOW};

use super::App;
use crate::i18n::Lang;
use crate::theme::{primary_button, radius, BLUE, HOVER, LINE, LINE_DARK, MUTED, PANEL, SEL, SIZE_BODY, SIZE_ICON, SIZE_SMALL, SIZE_TITLE, SLOT, STATE_OFF, TEXT};

const LAYER_ROW: f32 = 58.0;
const HISTORY_ROW: f32 = 74.0;
const LAYER_THUMB: f32 = 48.0;
/// Space for the longest value in the property rows.
const VALUE_WIDTH: f32 = 66.0;
const PROP_LABEL_WIDTH: f32 = 64.0;
const ACTION_HEIGHT: f32 = 48.0;
const PROP_GAP: f32 = 10.0;

fn fit_thumb(painter: &egui::Painter, tex: TextureId, size: Vec2, slot: Rect, tint: Color32) {
    let k = (slot.width() / size.x).min(slot.height() / size.y);
    let thumb = Rect::from_center_size(slot.center(), size * k);
    painter.image(tex, thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
    painter.rect_stroke(thumb, egui::CornerRadius::ZERO, Stroke::new(1.0, LINE_DARK), StrokeKind::Outside);
}

impl App {
    pub(super) fn right_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        // The end of the work, level with Generate on the other side.
        let action = Frame::new().fill(PANEL).inner_margin(Margin { left: 0, right: 0, top: 10, bottom: 2 });
        egui::Panel::bottom("export_action").frame(action).show(ui, |ui| {
            let has_result = self.current.is_some();
            let can_export = has_result && self.export_rx.is_none() && !self.export_pending;
            let main_action = has_result && !self.running() && self.unapplied(t).is_empty();
            let export = primary_button(format!("{}  {}", ph::DOWNLOAD_SIMPLE, t.export), SIZE_TITLE, main_action);
            let w = ui.available_width();
            if ui.add_enabled_ui(can_export, |ui| ui.add_sized([w, ACTION_HEIGHT], export)).inner.on_hover_text(t.export_tip).clicked() {
                self.export_open = true;
            }
        });
        ui.horizontal(|ui| {
            for (i, label) in [t.tab_layers, t.history].into_iter().enumerate() {
                if ui.add(egui::Button::new(label).selected(self.right_tab == i).min_size(Vec2::new(96.0, 26.0))).clicked() {
                    self.right_tab = i;
                }
            }
        });
        ui.separator();
        match self.right_tab {
            0 => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.layers(ui));
            }
            _ => self.history(ui),
        }
    }

    /// Eye click: toggle. Alt+click: this layer alone; on the same layer again, back to what was visible before.
    fn click_eye(&mut self, id: usize, alt: bool) {
        if !alt {
            self.visible[id] = !self.visible[id];
            self.solo = None;
            return;
        }
        match self.solo {
            Some((layer, before)) if layer == id => {
                self.visible = before;
                self.solo = None;
            }
            _ => {
                let before = self.solo.map_or(self.visible, |(_, before)| before);
                self.solo = Some((id, before));
                self.visible = std::array::from_fn(|i| i == id);
                self.selected_layer = id;
            }
        }
    }

    /// Image-editor style layer list: eye = visibility (Alt+click: only this one), row = selection; the selected
    /// layer's blend mode and opacity are edited above the list.
    fn layers(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let has_result = self.views.is_some();
        let ids: Vec<usize> = if has_result {
            vec![SHADOW, AI, ORIGINAL]
        } else if self.image.is_some() {
            vec![ORIGINAL]
        } else {
            vec![]
        };
        let sel = if ids.contains(&self.selected_layer) { self.selected_layer } else { ids.first().copied().unwrap_or(ORIGINAL) };

        let field = ui.available_width() - PROP_LABEL_WIDTH - PROP_GAP;
        egui::Grid::new("layer_props").num_columns(2).spacing([PROP_GAP, 8.0]).min_col_width(PROP_LABEL_WIDTH).show(ui, |ui| {
            ui.label(t.blend_title);
            ui.add_enabled_ui(has_result && sel == SHADOW, |ui| {
                let text = if sel == SHADOW { t.blends[self.cfg.blend] } else { t.normal };
                egui::ComboBox::from_id_salt("blend").width(field).selected_text(text).show_ui(ui, |ui| {
                    for i in MENU_ORDER {
                        ui.selectable_value(&mut self.cfg.blend, i, t.blends[i]);
                    }
                });
            });
            ui.end_row();
            let label = ui.add(egui::Label::new(t.opacity).sense(Sense::click()));
            if label.on_hover_text(t.reset_tip).double_clicked() && has_result && sel != ORIGINAL {
                self.opacity[sel] = 1.0;
            }
            ui.add_enabled_ui(has_result && sel != ORIGINAL, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = field - VALUE_WIDTH - ui.spacing().item_spacing.x;
                    ui.add(egui::Slider::new(&mut self.opacity[sel], 0.0..=1.0).show_value(false));
                    let mut percent = self.opacity[sel] * 100.0;
                    if ui.add_sized([VALUE_WIDTH, 22.0], egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(1.0).fixed_decimals(0).suffix("%")).changed() {
                        self.opacity[sel] = percent / 100.0;
                    }
                });
            });
            ui.end_row();
            let label = ui.add(egui::Label::new(t.position).sense(Sense::click()));
            let moved = self.entry(self.current).map_or((0, 0), |e| e.offset);
            if label.on_hover_text(t.position_tip).double_clicked() {
                self.move_shadow(-moved.0, -moved.1);
            }
            ui.add_enabled_ui(has_result && sel == SHADOW, |ui| {
                ui.horizontal(|ui| {
                    let (mut x, mut y) = moved;
                    let w = (field - ui.spacing().item_spacing.x) / 2.0;
                    let dx = ui.add_sized([w, 22.0], egui::DragValue::new(&mut x).speed(1.0).prefix("X "));
                    let dy = ui.add_sized([w, 22.0], egui::DragValue::new(&mut y).speed(1.0).prefix("Y "));
                    if dx.changed() || dy.changed() {
                        self.move_shadow(x - moved.0, y - moved.1);
                    }
                });
            });
            ui.end_row();
        });
        ui.separator();

        let mut select = None;
        let mut eye: Option<(usize, bool)> = None;
        let mut toggle_move = false;
        let entry_thumbs: Option<[(TextureId, Vec2); 3]> =
            self.entry(self.current).map(|e| std::array::from_fn(|i| (e.layer_thumbs[i].id(), e.layer_thumbs[i].size_vec2())));
        for &id in &ids {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), LAYER_ROW), Sense::click());
            if sel == id {
                ui.painter().rect_filled(r, radius(), SEL);
                ui.painter().rect_stroke(r, radius(), Stroke::new(1.0, BLUE), StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, radius(), HOVER);
            }
            let visible = self.visible[id];
            // Eye (registered after the row, so it gets its own clicks).
            let eye_rect = Rect::from_min_size(Pos2::new(r.left() + 4.0, r.center().y - 14.0), Vec2::splat(28.0));
            let eye_resp = ui.interact(eye_rect, Id::new(("eye", id)), Sense::click());
            ui.painter().rect_filled(eye_rect, radius(), if eye_resp.hovered() { HOVER } else { SLOT });
            ui.painter().rect_stroke(eye_rect, radius(), Stroke::new(1.0, LINE), StrokeKind::Inside);
            let (icon, icon_color) = if visible { (ph::EYE, TEXT) } else { (ph::EYE_SLASH, STATE_OFF) };
            ui.painter().text(eye_rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(SIZE_ICON), icon_color);
            let label = format!("{}: {}", if visible { t.hide } else { t.show }, t.layer_names[id]);
            if eye_resp.on_hover_text(format!("{label}\n{}", t.eye_tip)).clicked() {
                eye = Some((id, ui.input(|i| i.modifiers.alt)));
            } else if resp.clicked() {
                select = Some(id);
            }
            // Thumbnail.
            let slot = Rect::from_center_size(Pos2::new(eye_rect.right() + 8.0 + LAYER_THUMB / 2.0, r.center().y), Vec2::splat(LAYER_THUMB));
            let tint = if visible { Color32::WHITE } else { Color32::from_white_alpha(110) };
            if let Some(thumbs) = &entry_thumbs {
                fit_thumb(ui.painter(), thumbs[id].0, thumbs[id].1, slot, tint);
            } else if let Some(img) = &self.image {
                fit_thumb(ui.painter(), img.tex.id(), Vec2::new(img.img.w as f32, img.img.h as f32), slot, tint);
            }
            // Name and blend / opacity.
            let x = slot.right() + 10.0;
            let painter = ui.painter_at(r.shrink(2.0));
            painter.text(Pos2::new(x, r.top() + 20.0), Align2::LEFT_CENTER, t.layer_names[id], FontId::proportional(SIZE_BODY), if visible { TEXT } else { MUTED });
            let sub = match id {
                ORIGINAL => t.background.to_string(),
                _ => {
                    let mode = if id == SHADOW { t.blends[self.cfg.blend] } else { t.normal };
                    format!("{mode} · {:.0}%", self.opacity[id] * 100.0)
                }
            };
            painter.text(Pos2::new(x, r.top() + 40.0), Align2::LEFT_CENTER, sub, FontId::proportional(SIZE_SMALL), MUTED);
            if id == ORIGINAL {
                let lock = Pos2::new(r.right() - 16.0, r.center().y);
                painter.text(lock, Align2::CENTER_CENTER, ph::LOCK_SIMPLE, FontId::proportional(14.0), MUTED);
                ui.interact(Rect::from_center_size(lock, Vec2::splat(18.0)), Id::new("lock"), Sense::hover()).on_hover_text(t.locked);
            }
            if id == SHADOW {
                // Only the shadow can be moved; the original stays where it is.
                let rect = Rect::from_min_size(Pos2::new(r.right() - 32.0, r.center().y - 14.0), Vec2::splat(28.0));
                let button = ui.interact(rect, Id::new("move_shadow"), Sense::click());
                let (fill, icon_color) = match (self.move_mode, button.hovered()) {
                    (true, _) => (BLUE, Color32::WHITE),
                    (false, true) => (HOVER, TEXT),
                    (false, false) => (SLOT, TEXT),
                };
                ui.painter().rect_filled(rect, radius(), fill);
                ui.painter().rect_stroke(rect, radius(), Stroke::new(1.0, if self.move_mode { BLUE } else { LINE }), StrokeKind::Inside);
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, ph::ARROWS_OUT_CARDINAL, FontId::proportional(SIZE_ICON), icon_color);
                toggle_move = button.on_hover_text(t.move_tip).clicked();
            }
        }
        if let Some(id) = select {
            self.selected_layer = id;
        }
        if let Some((id, alt)) = eye {
            self.click_eye(id, alt);
        }
        if toggle_move {
            self.set_move_mode(!self.move_mode);
        }
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let mut clicked = None;
        let mut use_seed = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in &self.history {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HISTORY_ROW), Sense::click());
                if self.current == Some(e.id) {
                    ui.painter().rect_filled(r, radius(), SEL);
                    ui.painter().rect_stroke(r, radius(), Stroke::new(1.0, BLUE), StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, radius(), HOVER);
                }
                let slot = Rect::from_min_size(r.min + Vec2::new(4.0, 4.0), Vec2::new(37.0, 66.0));
                let ts = e.thumb.size_vec2();
                let k = (slot.width() / ts.x).min(slot.height() / ts.y);
                ui.painter().image(e.thumb.id(), Rect::from_center_size(slot.center(), ts * k), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                let x = slot.right() + 8.0;
                let painter = ui.painter_at(r.shrink(2.0));
                let run = if self.lang == Lang::Ja { format!("{} {}", e.id, t.run_n) } else { format!("{} {}", t.run_n, e.id) };
                painter.text(Pos2::new(x, r.top() + 16.0), Align2::LEFT_CENTER, format!("{run} · {}", e.input.name), FontId::proportional(SIZE_BODY), TEXT);
                let p = &e.result.params;
                let line2 = if e.result.extracted { t.extract } else { t.light_short[p.light_dir] };
                painter.text(Pos2::new(x, r.top() + 37.0), Align2::LEFT_CENTER, line2, FontId::proportional(SIZE_SMALL), MUTED);
                let sub = format!("× {:.2} · {}", p.strength, if p.background { t.with_bg } else { t.subject_only });
                painter.text(Pos2::new(x, r.top() + 55.0), Align2::LEFT_CENTER, sub, FontId::proportional(SIZE_SMALL), MUTED);
                let tip = format!("{}  ·  {} {}  ·  {:.1} s", e.input.name, t.seed, p.seed, e.result.seconds);
                let resp = resp.on_hover_text(tip);
                if !e.result.extracted {
                    resp.context_menu(|ui| {
                        if ui.button(format!("{}: {}", t.use_seed, p.seed)).clicked() {
                            use_seed = Some(p.seed);
                            ui.close();
                        }
                    });
                }
                if resp.clicked() {
                    clicked = Some(e.id);
                }
            }
        });
        if let Some(id) = clicked {
            self.select_entry(id);
        }
        if let Some(seed) = use_seed {
            self.seed = seed.to_string();
        }
    }
}
