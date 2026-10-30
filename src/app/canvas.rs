//! The canvas: pan, zoom, compare, and the overlays on it. What is drawn here is how the user judges the shadows, so
//! scale means device pixels per image pixel in everything the user sees.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;

use kage::comfy;
use kage::models::MODELS;

use super::settings::open_in_explorer;
use super::{App, Screen, MODELS_TAB};
use crate::theme::{checkerboard, icon_button, primary_button, radius, BLUE, CONTROL_HEIGHT, GAP, GREEN, LINE_DARK, MUTED, PADDING_I, PANEL, RED, SECTION_GAP, SIZE_BODY, SIZE_ICON, SIZE_SMALL, SIZE_TITLE, TEXT};

/// Step of the zoom buttons and keys.
pub(super) const ZOOM_STEP: f32 = 1.25;
const SETUP_WIDTH: f32 = 460.0;
const SETUP_HEIGHT: f32 = 320.0;
/// Column of the step numbers in the setup guide.
const STEP_MARK: f32 = 20.0;
/// Room for the zoom value, so the buttons around it stay put.
const ZOOM_VALUE_WIDTH: f32 = 44.0;
const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.05..=16.0;
/// Canvas margin around the fitted image.
const FIT_MARGIN: f32 = 48.0;
/// From this many device pixels per image pixel, single pixels are shown as squares.
const NEAREST_FROM: f32 = 2.0;
/// Wheel zoom stops at actual pixels when it comes this close.
const ACTUAL_SNAP: f32 = 0.03;

fn snap_to_device(p: Pos2, ppp: f32) -> Pos2 {
    Pos2::new((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

impl App {
    fn zoom_to(&mut self, new_scale: f32) {
        let new_scale = new_scale.clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end());
        // Keeps the canvas center where it is.
        self.pan *= new_scale / self.scale;
        self.zoom = Some(new_scale);
    }

    pub(super) fn zoom_by(&mut self, factor: f32) {
        self.zoom_to(self.scale * factor);
    }

    pub(super) fn zoom_fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    pub(super) fn zoom_actual(&mut self, ctx: &egui::Context) {
        self.zoom_to(1.0 / ctx.pixels_per_point());
    }

    pub(super) fn canvas(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        resp.context_menu(|ui| {
            ui.label(RichText::new(t.canvas_shade).small().color(MUTED));
            for (i, name) in t.canvas_shades.iter().enumerate() {
                if ui.selectable_label(self.cfg.canvas_shade == i, *name).clicked() {
                    self.cfg.canvas_shade = i;
                    ui.close();
                }
            }
        });

        let Some(img) = self.image.clone() else {
            // Empty and plain, unless something still has to be set up.
            if self.setup_tab().is_some() {
                self.setup_guide(ui, rect);
            }
            self.drop_outline(&ctx, &painter, rect);
            return;
        };
        let has_result = self.views.is_some();

        // Pan (drag) and zoom (wheel, around the cursor).
        let size = Vec2::new(img.img.w as f32, img.img.h as f32);
        let fit = ((rect.width() - FIT_MARGIN) / size.x).min((rect.height() - FIT_MARGIN) / size.y);
        let mut scale = self.zoom.unwrap_or(fit);
        let image_rect = |scale: f32, pan: Vec2| Rect::from_min_size(snap_to_device(rect.center() + pan - size * scale / 2.0, ppp), size * scale);
        let mut divider_drag = false;
        let img_rect = image_rect(scale, self.pan);
        let compare_active = has_result && self.compare_on && !self.peek_original;
        if compare_active {
            let divider_x = img_rect.left() + img_rect.width() * self.compare;
            let handle = Rect::from_x_y_ranges(divider_x - 8.0..=divider_x + 8.0, img_rect.y_range());
            let h = ui.interact(handle, Id::new("divider"), Sense::drag());
            if h.hovered() || h.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if h.dragged() {
                divider_drag = true;
                self.compare = (self.compare + h.drag_delta().x / img_rect.width()).clamp(0.0, 1.0);
            }
        }
        let moving = self.move_mode && has_result;
        if moving && (resp.hovered() || resp.dragged()) {
            ctx.set_cursor_icon(egui::CursorIcon::Move);
        }
        if resp.dragged() && !divider_drag {
            if moving {
                self.move_rest += resp.drag_delta() / scale;
                let whole = Vec2::new(self.move_rest.x.trunc(), self.move_rest.y.trunc());
                self.move_rest -= whole;
                self.move_shadow(whole.x as i32, whole.y as i32);
            } else {
                self.pan += resp.drag_delta();
            }
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let mut new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end());
                if (new_scale * ppp - 1.0).abs() < ACTUAL_SNAP {
                    new_scale = 1.0 / ppp;
                }
                if let Some(pointer) = resp.hover_pos() {
                    let anchor = pointer - (rect.center() + self.pan);
                    self.pan -= anchor * (new_scale / scale - 1.0);
                }
                scale = new_scale;
                self.zoom = Some(scale);
            }
        }
        self.scale = scale;
        self.nearest = scale * ppp >= NEAREST_FROM;
        let img_rect = image_rect(scale, self.pan);
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.rect_filled(img_rect.expand(1.0), CornerRadius::ZERO, Color32::from_black_alpha(40));
        match self.views.as_ref().filter(|v| v.shown.is_some() && !self.peek_original) {
            Some(v) => {
                // Hidden layers can leave the stack transparent.
                checkerboard(&painter, img_rect.intersect(rect));
                // Compare: the original left of the divider, the shaded result right of it.
                let split = if compare_active { self.compare } else { 0.0 };
                let x = img_rect.left() + img_rect.width() * split;
                let right = Rect::from_min_max(Pos2::new(x, img_rect.top()), img_rect.max);
                painter.image(v.stack.id(), right, Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0)), Color32::WHITE);
                if compare_active {
                    let left = Rect::from_min_max(img_rect.min, Pos2::new(x, img_rect.bottom()));
                    painter.image(img.tex.id(), left, Rect::from_min_max(Pos2::ZERO, Pos2::new(split, 1.0)), Color32::WHITE);
                    let line = [Pos2::new(x, img_rect.top()), Pos2::new(x, img_rect.bottom())];
                    painter.line_segment(line, Stroke::new(3.0, Color32::from_black_alpha(120)));
                    painter.line_segment(line, Stroke::new(1.0, Color32::WHITE));
                    let knob = Rect::from_center_size(Pos2::new(x, img_rect.center().y), Vec2::new(22.0, 34.0));
                    painter.rect_filled(knob, radius(), Color32::WHITE);
                    painter.rect_stroke(knob, radius(), Stroke::new(1.0, MUTED), StrokeKind::Inside);
                    painter.text(knob.center(), Align2::CENTER_CENTER, ph::ARROWS_LEFT_RIGHT, FontId::proportional(14.0), TEXT);
                    let top = img_rect.top().max(rect.top()) + 6.0;
                    side_label(&painter, Pos2::new(x - 8.0, top), Align2::RIGHT_TOP, t.layer_names[kage::blend::ORIGINAL]);
                    side_label(&painter, Pos2::new(x + 8.0, top), Align2::LEFT_TOP, t.shaded_label);
                }
            }
            None => {
                if img.img.has_alpha {
                    checkerboard(&painter, img_rect.intersect(rect));
                }
                painter.image(img.tex.id(), img_rect, full_uv, Color32::WHITE);
            }
        }
        self.drop_outline(&ctx, &painter, rect);
        if moving {
            side_label(&painter, rect.left_top() + Vec2::splat(8.0), Align2::LEFT_TOP, t.moving_shadow);
        }
        self.toast_overlay(&ctx, rect);
    }

    /// First run: what is needed, in order. Each step has its action right here; the next one to do is the main action.
    fn setup_guide(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let t = self.t();
        let block = Rect::from_center_size(rect.center(), Vec2::new(SETUP_WIDTH.min(rect.width() - 24.0), SETUP_HEIGHT));
        ui.scope_builder(egui::UiBuilder::new().max_rect(block), |ui| {
            let (text, muted) = if self.cfg.canvas_shade == 0 { (TEXT, MUTED) } else { (Color32::WHITE, Color32::from_gray(0xd0)) };
            ui.visuals_mut().override_text_color = Some(text);
            ui.label(RichText::new(t.setup_title).strong().size(SIZE_TITLE));
            ui.label(RichText::new(t.setup_requirements).color(muted));
            ui.add_space(SECTION_GAP);

            let comfy_state = self.comfy.state();
            let comfy_done = matches!(comfy_state, comfy::State::Ready { .. } | comfy::State::Starting);
            let models_done = self.verifier.all_ok();
            let step = |ui: &mut egui::Ui, n: usize, done: bool, active: bool, title: &str, detail: &str| {
                let color = if done || active { text } else { muted };
                let mark = if done { ph::CHECK_CIRCLE.to_string() } else { n.to_string() };
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(Vec2::splat(STEP_MARK), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_width(STEP_MARK);
                        ui.label(RichText::new(mark).color(if done { GREEN } else { color }).strong());
                    });
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).strong().color(color));
                        ui.label(RichText::new(detail).color(muted));
                    });
                });
            };
            let indent = |ui: &mut egui::Ui| ui.add_space(STEP_MARK + GAP);

            let comfy_detail = match comfy_state {
                comfy::State::NotSet => t.step_comfy_detail,
                comfy::State::NotFound => t.comfy_states[1],
                comfy::State::MissingNodes(_) => t.comfy_states[4],
                comfy::State::Failed(_) => t.comfy_states[5],
                comfy::State::Starting => t.comfy_states[2],
                comfy::State::Ready { .. } => t.model_ok,
            };
            step(ui, 1, comfy_done, true, t.step_comfy, comfy_detail);
            if !comfy_done {
                ui.horizontal(|ui| {
                    indent(ui);
                    if ui.add(primary_button(t.choose_dir, SIZE_BODY, true)).clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.cfg.comfy_dir = dir.display().to_string();
                            self.apply_dirs();
                        }
                    }
                    if ui.button(format!("{}  {}", ph::LINK, t.go_to_source)).on_hover_text(super::settings::COMFY_RELEASES).clicked() {
                        open_in_explorer(std::path::Path::new(super::settings::COMFY_RELEASES));
                    }
                });
            }
            ui.add_space(SECTION_GAP);

            let present = self.verifier.statuses().iter().filter(|s| **s == kage::models::Status::Ok).count();
            let mut detail = t.step_models_detail.replace("{total}", &MODELS.len().to_string()).replace("{done}", &present.to_string());
            if !models_done {
                detail = format!("{detail}\n{}", t.step_models_hint);
            }
            step(ui, 2, models_done, comfy_done, t.step_models, &detail);
            if !models_done {
                ui.horizontal(|ui| {
                    indent(ui);
                    if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_models)).on_hover_text(self.cfg.model_dir.as_str()).clicked() {
                        open_in_explorer(std::path::Path::new(self.cfg.model_dir.trim()));
                    }
                    let in_comfy = self.comfy_models_to_use();
                    if let Some(dir) = &in_comfy {
                        if ui.add(primary_button(t.use_models, SIZE_BODY, true)).on_hover_text(dir.display().to_string()).clicked() {
                            self.cfg.model_dir = dir.display().to_string();
                            self.apply_dirs();
                        }
                    }
                    if ui.add(primary_button(t.set_models, SIZE_BODY, comfy_done && in_comfy.is_none())).clicked() {
                        self.screen = Screen::Settings;
                        self.settings_tab = MODELS_TAB;
                    }
                });
            }
            ui.add_space(SECTION_GAP);

            step(ui, 3, false, false, t.step_image, t.step_image_detail);
        });
    }

    /// Compare and zoom, under the canvas they act on.
    pub(super) fn view_tools(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let has_image = self.image.is_some();
        let has_result = self.current.is_some();
        ui.horizontal_centered(|ui| {
            ui.add_space(((ui.available_width() - self.view_tools_width) / 2.0).max(0.0));
            let left = ui.cursor().left();
            let compare = egui::Button::new(format!("{}  {}", ph::SQUARE_SPLIT_HORIZONTAL, t.compare)).selected(has_result && self.compare_on);
            if ui.add_enabled(has_result, compare).on_hover_text(t.compare_tip).clicked() {
                self.compare_on = !self.compare_on;
            }
            ui.separator();
            ui.add_enabled_ui(has_image, |ui| {
                if icon_button(ui, ph::MINUS).on_hover_text(t.zoom_out).clicked() {
                    self.zoom_by(1.0 / ZOOM_STEP);
                }
                let percent = if has_image { format!("{:.0}%", self.scale * ctx.pixels_per_point() * 100.0) } else { "-%".to_string() };
                ui.add_sized([ZOOM_VALUE_WIDTH, CONTROL_HEIGHT], egui::Label::new(RichText::new(percent).monospace()));
                if icon_button(ui, ph::PLUS).on_hover_text(t.zoom_in).clicked() {
                    self.zoom_by(ZOOM_STEP);
                }
                if ui.button(t.fit).on_hover_text(t.fit_tip).clicked() {
                    self.zoom_fit();
                }
                if ui.button("1:1").on_hover_text(t.actual_tip).clicked() {
                    self.zoom_actual(&ctx);
                }
            });
            self.view_tools_width = ui.cursor().left() - left - GAP;
        });
    }

    /// A file is being dragged over the window.
    fn drop_outline(&self, ctx: &egui::Context, painter: &egui::Painter, rect: Rect) {
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            painter.rect_stroke(rect.shrink(1.0), CornerRadius::ZERO, Stroke::new(2.0, BLUE), StrokeKind::Inside);
        }
    }

    /// Messages at the top of the canvas. Failures stay until closed.
    fn toast_overlay(&mut self, ctx: &egui::Context, rect: Rect) {
        let t = self.t();
        let overlay = Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE_DARK)).corner_radius(radius()).inner_margin(Margin::symmetric(PADDING_I, 4));
        let mut close = false;
        if let Some(toast) = &self.toast {
            let mut open_dir = None;
            egui::Area::new(Id::new("toast"))
                .fixed_pos(rect.center_top() + Vec2::new(0.0, SECTION_GAP))
                .pivot(Align2::CENTER_TOP)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.set_max_width(rect.width() - 32.0);
                    overlay.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = if toast.ok { (ph::CHECK_CIRCLE, GREEN) } else { (ph::WARNING_CIRCLE, RED) };
                            ui.label(RichText::new(icon).size(SIZE_ICON).color(color));
                            ui.add(egui::Label::new(&toast.text).wrap());
                            if let Some(dir) = &toast.folder {
                                if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_folder)).clicked() {
                                    open_dir = Some(dir.clone());
                                }
                            }
                            if !toast.ok && ui.button(t.close).clicked() {
                                close = true;
                            }
                        });
                    });
                });
            if let Some(dir) = open_dir {
                open_in_explorer(&dir);
            }
        }
        if close {
            self.toast = None;
        }
    }
}

/// Which side of the compare divider is which.
fn side_label(painter: &egui::Painter, pos: Pos2, align: Align2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(SIZE_SMALL), TEXT);
    let rect = align.anchor_size(pos, galley.size() + Vec2::new(10.0, 4.0));
    painter.rect_filled(rect, radius(), PANEL);
    painter.rect_stroke(rect, radius(), Stroke::new(1.0, LINE_DARK), StrokeKind::Inside);
    painter.galley(rect.min + Vec2::new(5.0, 2.0), galley, TEXT);
}
