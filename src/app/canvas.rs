//! The canvas: pan, zoom, compare, and the overlays on it. What is drawn here is how the user judges the shadows, so
//! scale means device pixels per image pixel in everything the user sees.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Frame, Id, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;

use super::settings::open_in_explorer;
use super::{App, Screen};
use crate::theme::{checkerboard, radius, BLUE, GREEN, LINE_DARK, MUTED, PANEL, RED, SIZE_BODY, SIZE_ICON, SIZE_SMALL, TEXT};

/// Step of the zoom buttons and keys.
pub(super) const ZOOM_STEP: f32 = 1.25;
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
        let on_canvas = |c: Color32| if self.cfg.canvas_shade == 0 { c } else { Color32::WHITE };

        let Some(img) = self.image.clone() else {
            // Empty and plain, unless something still has to be set up.
            if let Some(tab) = self.setup_tab() {
                painter.text(rect.center() - Vec2::new(0.0, 20.0), Align2::CENTER_CENTER, t.setup_needed, FontId::proportional(SIZE_BODY), on_canvas(TEXT));
                let button = Rect::from_center_size(rect.center() + Vec2::new(0.0, 14.0), Vec2::new(120.0, 28.0));
                if ui.put(button, egui::Button::new(t.open_settings)).clicked() {
                    self.screen = Screen::Settings;
                    self.settings_tab = tab;
                }
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
        if resp.dragged() && !divider_drag {
            self.pan += resp.drag_delta();
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
        self.toast_overlay(&ctx, rect);
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
        let overlay = Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE_DARK)).corner_radius(radius()).inner_margin(Margin::symmetric(6, 4));
        let mut close = false;
        if let Some(toast) = &self.toast {
            let mut open_dir = None;
            egui::Area::new(Id::new("toast"))
                .fixed_pos(rect.center_top() + Vec2::new(0.0, 12.0))
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
