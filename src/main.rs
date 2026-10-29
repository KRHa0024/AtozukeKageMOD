//! Atozuke Kage MOD — UI prototype (egui). No backend yet: generation is simulated, the shading preview is a
//! uniform darkening of the opened image.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod i18n;

use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextureHandle, Vec2,
};
use egui_phosphor::regular as ph;
use i18n::{t, Lang, T};

const BLUE: Color32 = Color32::from_rgb(0x00, 0x53, 0x7d);
const SEL: Color32 = Color32::from_rgb(0xbf, 0xe3, 0xff);
const PANEL: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const BG: Color32 = Color32::from_rgb(0xf8, 0xf8, 0xf8);
const CANVAS: Color32 = Color32::from_rgb(0xe3, 0xe3, 0xe3);
const LINE: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
const LINE_DARK: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const TEXT: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
const MUTED: Color32 = Color32::from_rgb(0x6e, 0x6e, 0x6e);
const GREEN: Color32 = Color32::from_rgb(0x2e, 0x8b, 0x57);
const AMBER: Color32 = Color32::from_rgb(0x8a, 0x5a, 0x08);
const RED: Color32 = Color32::from_rgb(0xb0, 0x3a, 0x2e);

/// Simulated run length of the prototype; the status shows the times of a real 4K run.
const DEMO_SECONDS: f64 = 16.0;
const REAL_SECONDS: f64 = 64.8;
/// Progress fractions where each stage ends (AI shading, subject, layer, save).
const STAGE_ENDS: [f64; 4] = [0.78, 0.84, 0.96, 1.0];

const DIR_ICONS: [&str; 9] = [
    ph::ARROW_DOWN_RIGHT,
    ph::ARROW_DOWN,
    ph::ARROW_DOWN_LEFT,
    ph::ARROW_RIGHT,
    ph::SUN,
    ph::ARROW_LEFT,
    ph::ARROW_UP_RIGHT,
    ph::ARROW_UP,
    ph::ARROW_UP_LEFT,
];

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Main,
    Settings,
}

#[derive(PartialEq, Clone, Copy)]
enum LayerView {
    Result,
    Multiply,
    Normal,
    Ai,
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Running { started: f64 },
    Done { at: f64 },
    Stopped,
}

struct LoadedImage {
    tex: TextureHandle,
    size: [usize; 2],
    name: String,
}

struct App {
    lang: Lang,
    screen: Screen,
    settings_tab: usize,
    image: Option<LoadedImage>,
    load_error: Option<String>,
    dir: usize,
    strength: f32,
    bg_shadow: bool,
    gray: bool,
    extra: String,
    ghost: bool,
    seed: String,
    phase: Phase,
    stage: usize,
    stage_changed_at: f64,
    layer_menu_open: bool,
    layer_view: LayerView,
    blend: usize,
    opacity: f32,
    compare_on: bool,
    compare: f32,
    zoom: Option<f32>,
    pan: Vec2,
    history_open: bool,
    history_sel: usize,
    export_open: bool,
    targets: [bool; 3],
    format: usize,
    png_level: u8,
    quality: u8,
    out_dir: String,
    model_dir: String,
    comfy_dir: String,
    rechecked: bool,
    keep_loaded: bool,
    log_level: usize,
    adv_default_open: bool,
    shot: Option<Shot>,
}

/// Self-screenshot for checking the layout: KAGE_SHOT=<out.png> KAGE_SCENE=<scene> [KAGE_IMAGE=<path>] [KAGE_LANG=en].
struct Shot {
    out: std::path::PathBuf,
    frame: u32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        Self {
            lang: Lang::Ja,
            screen: Screen::Main,
            settings_tab: 0,
            image: None,
            load_error: None,
            dir: 0,
            strength: 1.0,
            bg_shadow: false,
            gray: false,
            extra: String::new(),
            ghost: false,
            seed: "random".into(),
            phase: Phase::Idle,
            stage: 0,
            stage_changed_at: 0.0,
            layer_menu_open: false,
            layer_view: LayerView::Result,
            blend: 0,
            opacity: 1.0,
            compare_on: true,
            compare: 0.5,
            zoom: None,
            pan: Vec2::ZERO,
            history_open: false,
            history_sel: 0,
            export_open: false,
            targets: [true, false, true],
            format: 0,
            png_level: 6,
            quality: 92,
            out_dir: r"C:\Users\(user)\Pictures\AtozukeKageMOD".into(),
            model_dir: r"D:\AtozukeKageMOD\models".into(),
            comfy_dir: r"D:\ComfyUI_windows_portable".into(),
            rechecked: false,
            keep_loaded: true,
            log_level: 0,
            adv_default_open: false,
            shot: None,
        }
        .with_scene(&cc.egui_ctx)
    }

    fn with_scene(mut self, ctx: &egui::Context) -> Self {
        let Ok(out) = std::env::var("KAGE_SHOT") else { return self };
        self.shot = Some(Shot { out: out.into(), frame: 0 });
        if std::env::var("KAGE_LANG").as_deref() == Ok("en") {
            self.lang = Lang::En;
        }
        let scene = std::env::var("KAGE_SCENE").unwrap_or_default();
        if scene != "empty" {
            if let Ok(path) = std::env::var("KAGE_IMAGE") {
                self.open_path(ctx, std::path::Path::new(&path));
                self.phase = Phase::Done { at: -10.0 };
            }
        }
        match scene.as_str() {
            "running" => {
                self.phase = Phase::Running { started: -0.4 * DEMO_SECONDS };
                self.stage = 0;
                self.stage_changed_at = -10.0;
            }
            "layers" => self.layer_menu_open = true,
            "advanced" => self.adv_default_open = true,
            "history" => self.history_open = true,
            "export" => self.export_open = true,
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

    fn handle_shot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        shot.frame += 1;
        if shot.frame == 15 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
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
    }

    fn t(&self) -> &'static T {
        t(self.lang)
    }

    fn open_path(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                let tex = ctx.load_texture("input", color, egui::TextureOptions::LINEAR);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.image = Some(LoadedImage { tex, size, name });
                self.load_error = None;
                self.zoom = None;
                self.pan = Vec2::ZERO;
                self.phase = Phase::Idle;
            }
            Err(e) => self.load_error = Some(e.to_string()),
        }
    }

    fn pick_image(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            self.open_path(ctx, &path);
        }
    }

    /// Progress 0..1 of the simulated run.
    fn progress(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Running { started } => ((now - started) / DEMO_SECONDS).clamp(0.0, 1.0),
            Phase::Done { .. } => 1.0,
            _ => 0.0,
        }
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Phase::Running { .. } = self.phase {
            let p = self.progress(now);
            let stage = STAGE_ENDS.iter().position(|&end| p < end).unwrap_or(3);
            if stage != self.stage {
                self.stage = stage;
                self.stage_changed_at = now;
            }
            if p >= 1.0 {
                self.phase = Phase::Done { at: now };
            }
            ctx.request_repaint();
        }
        if let Phase::Done { at } = self.phase {
            if now - at < 0.5 {
                ctx.request_repaint();
            }
        }
    }

    // ---- main screen -------------------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(t.app_name).strong().color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button(format!("{}  {}", ph::GEAR_SIX, t.settings))
                    .on_hover_text(t.settings_tip)
                    .clicked()
                {
                    self.screen = Screen::Settings;
                }
                ui.label(RichText::new(format!("●  {}", t.model_ok)).color(GREEN))
                    .on_hover_text(t.model_ok_tip);
                self.language_combo(ui);
                ui.separator();
                if ui
                    .add(Button::icon_text(ph::CLOCK_COUNTER_CLOCKWISE, t.history).selected(self.history_open))
                    .on_hover_text(t.history_tip)
                    .clicked()
                {
                    self.history_open = !self.history_open;
                }
                if ui
                    .add_enabled(self.image.is_some(), Button::icon_text(ph::DOWNLOAD_SIMPLE, t.export))
                    .on_hover_text(t.export_tip)
                    .clicked()
                {
                    self.export_open = true;
                }
            });
        });
    }

    fn language_combo(&mut self, ui: &mut egui::Ui) {
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
        ui.label(RichText::new(ph::GLOBE).size(16.0)).on_hover_text(t.language_tip);
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let running = matches!(self.phase, Phase::Running { .. });

        egui::Panel::bottom("generate")
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(0, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 84.0 - ui.spacing().item_spacing.x;
                    let gen = egui::Button::new(
                        RichText::new(format!("{}  {}", ph::PLAY, t.generate)).size(16.0).strong().color(BLUE),
                    )
                    .fill(SEL)
                    .stroke(Stroke::new(1.0, BLUE));
                    if ui
                        .add_enabled_ui(self.image.is_some() && !running, |ui| ui.add_sized([w, 48.0], gen))
                        .inner
                        .on_hover_text(t.generate_tip)
                        .clicked()
                    {
                        let now = ctx.input(|i| i.time);
                        self.phase = Phase::Running { started: now };
                        self.stage = 0;
                        self.stage_changed_at = now;
                        self.layer_view = LayerView::Result;
                    }
                    let stop = egui::Button::new(RichText::new(format!("{}  {}", ph::STOP, t.stop)).size(14.0));
                    if ui
                        .add_enabled_ui(running, |ui| ui.add_sized([84.0, 48.0], stop))
                        .inner
                        .on_hover_text(t.stop_tip)
                        .clicked()
                    {
                        self.phase = Phase::Stopped;
                    }
                });
            });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(10.0);
            // Open / drop box
            let inner = Frame::new()
                .fill(Color32::from_rgb(0xfa, 0xfa, 0xfa))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let open = egui::Button::new(
                        RichText::new(format!("{}  {}", ph::FOLDER_OPEN, t.open_image)).size(15.0).strong(),
                    );
                    if ui.add_sized([ui.available_width(), 44.0], open).on_hover_text(t.open_tip).clicked() {
                        self.pick_image(&ctx);
                    }
                    ui.vertical_centered(|ui| ui.label(RichText::new(t.drop_hint).small().color(MUTED)));
                    if let Some(img) = &self.image {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&img.name).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{}×{}", img.size[0], img.size[1])).monospace().color(MUTED),
                                );
                            });
                        });
                    }
                    if let Some(err) = &self.load_error {
                        ui.label(RichText::new(err).small().color(RED));
                    }
                });
            dashed_rect(ui.painter(), inner.response.rect, Color32::from_gray(0x9a));

            ui.add_space(12.0);
            ui.label(RichText::new(t.light_dir).strong());
            let w = (ui.available_width() - 6.0) / 3.0;
            egui::Grid::new("dirs").spacing([3.0, 3.0]).show(ui, |ui| {
                for (i, icon) in DIR_ICONS.iter().enumerate() {
                    let b = egui::Button::new(RichText::new(*icon).size(17.0)).selected(self.dir == i);
                    if ui.add_sized([w, 34.0], b).on_hover_text(t.light_tips[i]).clicked() {
                        self.dir = i;
                    }
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t.strength).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("× {:.1}", self.strength)).monospace());
                });
            });
            ui.spacing_mut().slider_width = ui.available_width();
            ui.add(egui::Slider::new(&mut self.strength, 0.5..=1.5).step_by(0.1).show_value(false));

            ui.add_space(10.0);
            ui.checkbox(&mut self.bg_shadow, t.bg_shadow).on_hover_text(t.bg_tip);
            ui.checkbox(&mut self.gray, t.gray).on_hover_text(t.gray_tip);

            ui.add_space(8.0);
            ui.separator();
            egui::CollapsingHeader::new(RichText::new(t.advanced).strong())
                .id_salt("advanced")
                .default_open(self.adv_default_open)
                .show(ui, |ui| {
                    ui.label(t.extra);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.extra)
                            .hint_text("warm sunset light")
                            .desired_width(f32::INFINITY),
                    );
                    ui.label(RichText::new(t.extra_desc).small().color(MUTED));
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.ghost, t.ghost);
                    ui.label(RichText::new(t.ghost_desc).small().color(MUTED));
                    ui.add_space(8.0);
                    ui.label(t.seed);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.seed).desired_width(ui.available_width() - 34.0));
                        if ui.button(ph::ARROWS_CLOCKWISE).on_hover_text(t.seed_random).clicked() {
                            self.seed = "random".into();
                        }
                    });
                    ui.label(RichText::new(t.seed_desc).small().color(MUTED));
                    ui.add_space(8.0);
                    let b = egui::Button::new(format!("{}  {}", ph::SELECTION_FOREGROUND, t.extract));
                    ui.add_enabled(self.image.is_some(), b.min_size(Vec2::new(ui.available_width(), 28.0)));
                    ui.label(RichText::new(t.extract_desc).small().color(MUTED));
                });
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        let Some(img) = &self.image else {
            let inner = rect.shrink(24.0);
            dashed_rect(&painter, inner, Color32::from_gray(0x9a));
            painter.text(
                inner.center() - Vec2::new(0.0, 14.0),
                Align2::CENTER_CENTER,
                ph::UPLOAD_SIMPLE,
                FontId::proportional(40.0),
                MUTED,
            );
            painter.text(
                inner.center() + Vec2::new(0.0, 24.0),
                Align2::CENTER_CENTER,
                t.drop_here,
                FontId::proportional(15.0),
                MUTED,
            );
            return;
        };

        // Pan (drag) and zoom (wheel, around the cursor).
        let size = Vec2::new(img.size[0] as f32, img.size[1] as f32);
        let fit = ((rect.width() - 48.0) / size.x).min((rect.height() - 48.0) / size.y);
        let mut scale = self.zoom.unwrap_or(fit);
        let mut divider_drag = false;
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        let divider_x = img_rect.left() + img_rect.width() * self.compare;
        let compare_active = self.compare_on && self.layer_view == LayerView::Result;
        if compare_active {
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
                let new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(0.05, 8.0);
                if let Some(pointer) = resp.hover_pos() {
                    let anchor = pointer - (rect.center() + self.pan);
                    self.pan -= anchor * (new_scale / scale - 1.0);
                }
                scale = new_scale;
                self.zoom = Some(scale);
            }
        }
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.rect_filled(img_rect.expand(1.0), CornerRadius::ZERO, Color32::from_black_alpha(40));

        // Prototype shading: a uniform darkening stands in for the real shadow layer.
        let shade = (255.0 - 70.0 * self.strength * self.opacity).clamp(0.0, 255.0) as u8;
        let tint = if self.gray { Color32::from_gray(shade) } else { Color32::from_rgb(shade, shade.saturating_sub(12), shade.saturating_sub(4)) };
        match self.layer_view {
            LayerView::Result => {
                painter.image(img.tex.id(), img_rect, full_uv, Color32::WHITE);
                let split = if compare_active { self.compare } else { 1.0 };
                let shaded = Rect::from_min_max(img_rect.min, Pos2::new(img_rect.left() + img_rect.width() * split, img_rect.bottom()));
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(split, 1.0));
                painter.image(img.tex.id(), shaded, uv, tint);
                if compare_active {
                    let x = img_rect.left() + img_rect.width() * self.compare;
                    painter.line_segment([Pos2::new(x, img_rect.top()), Pos2::new(x, img_rect.bottom())], Stroke::new(2.0, Color32::WHITE));
                    let knob = Rect::from_center_size(Pos2::new(x, img_rect.center().y), Vec2::new(22.0, 34.0));
                    painter.rect_filled(knob, CornerRadius::same(2), Color32::WHITE);
                    painter.rect_stroke(knob, CornerRadius::same(2), Stroke::new(1.0, Color32::from_gray(0x8a)), StrokeKind::Inside);
                    painter.text(knob.center(), Align2::CENTER_CENTER, ph::ARROWS_LEFT_RIGHT, FontId::proportional(14.0), TEXT);
                }
            }
            LayerView::Multiply => {
                painter.rect_filled(img_rect, CornerRadius::ZERO, Color32::WHITE);
                painter.text(img_rect.center(), Align2::CENTER_CENTER, t.prototype_note, FontId::proportional(13.0), MUTED);
            }
            LayerView::Normal => {
                checkerboard(&painter, img_rect.intersect(rect));
                painter.text(img_rect.center(), Align2::CENTER_CENTER, t.prototype_note, FontId::proportional(13.0), MUTED);
            }
            LayerView::Ai => {
                painter.image(img.tex.id(), img_rect, full_uv, tint);
            }
        }

        self.canvas_overlays(&ctx, rect, scale, fit);
    }

    fn canvas_overlays(&mut self, ctx: &egui::Context, rect: Rect, scale: f32, fit: f32) {
        let t = self.t();
        let overlay = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, LINE_DARK))
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::symmetric(6, 4));

        // Layer menu (top left)
        egui::Area::new(Id::new("layer_menu"))
            .fixed_pos(rect.left_top() + Vec2::new(8.0, 8.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    let label = match self.layer_view {
                        LayerView::Result => format!("{}  {} · {}  {}", ph::STACK, t.views[0], t.blends[self.blend], ph::CARET_DOWN),
                        v => format!("{}  {}  {}", ph::STACK, t.views[v as usize], ph::CARET_DOWN),
                    };
                    if ui.add(egui::Button::new(label).frame(false)).on_hover_text(t.layer_tip).clicked() {
                        self.layer_menu_open = !self.layer_menu_open;
                    }
                    if self.layer_menu_open {
                        ui.separator();
                        ui.set_width(250.0);
                        ui.label(RichText::new(t.view).strong());
                        for (i, v) in [LayerView::Result, LayerView::Multiply, LayerView::Normal, LayerView::Ai].into_iter().enumerate() {
                            ui.radio_value(&mut self.layer_view, v, t.views[i]);
                        }
                        ui.separator();
                        ui.label(RichText::new(t.blend_title).strong());
                        egui::ComboBox::from_id_salt("blend")
                            .width(240.0)
                            .selected_text(t.blends[self.blend])
                            .show_ui(ui, |ui| {
                                for (i, name) in t.blends.iter().enumerate() {
                                    ui.selectable_value(&mut self.blend, i, *name);
                                }
                            });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t.opacity).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(format!("{:.0}%", self.opacity * 100.0)).monospace());
                            });
                        });
                        ui.spacing_mut().slider_width = 240.0;
                        ui.add(egui::Slider::new(&mut self.opacity, 0.0..=1.0).show_value(false));
                    }
                });
            });

        // Compare toggle (top right)
        egui::Area::new(Id::new("compare"))
            .fixed_pos(rect.right_top() + Vec2::new(-8.0, 8.0))
            .pivot(Align2::RIGHT_TOP)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    ui.checkbox(&mut self.compare_on, t.compare).on_hover_text(t.compare_tip);
                });
            });

        // Zoom (bottom right)
        egui::Area::new(Id::new("zoom"))
            .fixed_pos(rect.right_bottom() + Vec2::new(-8.0, -8.0))
            .pivot(Align2::RIGHT_BOTTOM)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("−").on_hover_text(t.zoom_out).clicked() {
                            self.zoom = Some((scale / 1.25).max(0.05));
                        }
                        ui.label(RichText::new(format!("{:>4.0}%", scale * 100.0)).monospace());
                        if ui.button("+").on_hover_text(t.zoom_in).clicked() {
                            self.zoom = Some((scale * 1.25).min(8.0));
                        }
                        if ui.button(t.fit).on_hover_text(t.fit_tip).clicked() {
                            self.zoom = None;
                            self.pan = Vec2::ZERO;
                        }
                        if ui.button("1:1").on_hover_text(t.actual_tip).clicked() {
                            self.zoom = Some(1.0);
                        }
                        let _ = fit;
                    });
                });
            });
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.history).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(ph::X).clicked() {
                    self.history_open = false;
                }
            });
        });
        ui.separator();
        let runs = [("1.0", 0usize, false), ("1.2", 2, false), ("1.0", 4, true), ("0.8", 3, false)];
        // "Auto (match the image's light)" -> "Auto": the drawer is narrow.
        let dir_short = |i: usize| {
            let s = t.light_tips[i].split(": ").last().unwrap_or("");
            s.split(['（', '(']).next().unwrap_or(s).trim_end()
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, (k, d, bg)) in runs.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 74.0), Sense::click());
                if self.history_sel == i {
                    ui.painter().rect_filled(r, CornerRadius::same(2), SEL);
                    ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, BLUE), StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(2), Color32::from_gray(0xe6));
                }
                let thumb = Rect::from_min_size(r.min + Vec2::new(4.0, 4.0), Vec2::new(37.0, 66.0));
                match &self.image {
                    Some(img) => {
                        ui.painter().image(img.tex.id(), thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().rect_filled(thumb, CornerRadius::ZERO, Color32::from_gray(0xb0));
                    }
                }
                let x = thumb.right() + 8.0;
                let painter = ui.painter_at(r.shrink(2.0));
                let title = if self.lang == Lang::Ja { format!("{} {}", runs.len() - i, t.run_n) } else { format!("{} {}", t.run_n, runs.len() - i) };
                painter.text(Pos2::new(x, r.top() + 18.0), Align2::LEFT_CENTER, title, FontId::proportional(13.0), TEXT);
                painter.text(Pos2::new(x, r.top() + 39.0), Align2::LEFT_CENTER, dir_short(*d), FontId::proportional(12.0), MUTED);
                let sub = format!("×{} · {}", k, if *bg { t.with_bg } else { t.subject_only });
                painter.text(Pos2::new(x, r.top() + 56.0), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), MUTED);
                if resp.clicked() {
                    self.history_sel = i;
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let now = ui.input(|i| i.time);
        let p = self.progress(now);
        let running = matches!(self.phase, Phase::Running { .. });
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            match self.phase {
                Phase::Running { .. } => {
                    ui.add(egui::Spinner::new().size(16.0).color(BLUE));
                }
                Phase::Done { at } => {
                    let k = ((now - at) / 0.35).clamp(0.0, 1.0) as f32;
                    let pop = if k < 0.7 { 0.4 + k / 0.7 * 0.75 } else { 1.15 - (k - 0.7) / 0.3 * 0.15 };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, ph::CHECK_CIRCLE, FontId::proportional(18.0 * pop), GREEN);
                }
                _ => {
                    ui.label(RichText::new(ph::CIRCLE).size(16.0).color(Color32::from_gray(0x9a)));
                }
            }
            if running {
                Frame::new()
                    .fill(Color32::from_rgb(0xdd, 0xe9, 0xf2))
                    .corner_radius(CornerRadius::same(2))
                    .inner_margin(Margin::symmetric(5, 1))
                    .show(ui, |ui| ui.label(RichText::new(format!("{}/4", self.stage + 1)).monospace().small().color(BLUE)));
            }

            // Stage text: slides in from below whenever the stage changes; dots cycle while running.
            let text = match self.phase {
                Phase::Running { .. } => t.running[self.stage],
                Phase::Done { .. } => t.done,
                Phase::Stopped => t.stopped,
                Phase::Idle => t.idle,
            };
            let font = FontId::proportional(13.0);
            let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), TEXT);
            let dots_w = 16.0;
            let (r, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + dots_w, 20.0), Sense::hover());
            let k = if running { ((now - self.stage_changed_at) / 0.35).clamp(0.0, 1.0) as f32 } else { 1.0 };
            let ease = 1.0 - (1.0 - k).powi(3);
            let color = TEXT.gamma_multiply(ease);
            let pos = Pos2::new(r.left(), r.center().y - galley.size().y / 2.0 + (1.0 - ease) * 6.0);
            ui.painter().galley(pos, galley.clone(), color);
            if running {
                for d in 0..3 {
                    let phase = ((now * 1.6 - d as f64 * 0.2).rem_euclid(1.2) / 1.2) as f32;
                    let a = if phase < 0.4 { 0.15 + phase / 0.4 * 0.85 } else { 1.0 - (phase - 0.4) / 0.6 * 0.85 };
                    let x = r.left() + galley.size().x + 2.0 + d as f32 * 4.5;
                    ui.painter().circle_filled(Pos2::new(x, r.center().y + 4.0), 1.3, TEXT.gamma_multiply(a));
                }
            }

            let detail = match self.phase {
                Phase::Running { .. } if p < STAGE_ENDS[0] => {
                    format!("{}/25 {}", ((p / STAGE_ENDS[0]) * 25.0).ceil().max(1.0) as u32, t.steps)
                }
                Phase::Running { .. } => format!("{:.0}%", p * 100.0),
                Phase::Done { .. } => self.image.as_ref().map(|i| format!("{}×{}", i.size[0], i.size[1])).unwrap_or_default(),
                _ => String::new(),
            };
            ui.label(RichText::new(detail).monospace().color(MUTED));
            ui.add_space(8.0);

            // One progress bar; a light band sweeps over the filled part while running.
            let bar_w = (ui.available_width() * 0.42).clamp(160.0, 520.0);
            let (bar, _) = ui.allocate_exact_size(Vec2::new(bar_w, 8.0), Sense::hover());
            ui.painter().rect_filled(bar, CornerRadius::same(1), Color32::from_gray(0xdc));
            let fill = Rect::from_min_size(bar.min, Vec2::new(bar.width() * p as f32, bar.height()));
            ui.painter().rect_filled(fill, CornerRadius::same(1), BLUE);
            if running && fill.width() > 2.0 {
                let band = 90.0;
                let x = fill.left() - band + ((now * 220.0) % (fill.width() as f64 + band as f64 * 2.0)) as f32;
                let mut mesh = egui::Mesh::default();
                let top = fill.top();
                let bottom = fill.bottom();
                let c0 = Color32::from_white_alpha(0);
                let c1 = Color32::from_white_alpha(110);
                for (i, (xx, c)) in [(x, c0), (x + band / 2.0, c1), (x + band, c0)].into_iter().enumerate() {
                    mesh.colored_vertex(Pos2::new(xx, top), c);
                    mesh.colored_vertex(Pos2::new(xx, bottom), c);
                    if i > 0 {
                        let b = (i as u32) * 2;
                        mesh.add_triangle(b - 2, b - 1, b);
                        mesh.add_triangle(b - 1, b, b + 1);
                    }
                }
                ui.painter().with_clip_rect(fill).add(egui::Shape::mesh(mesh));
            }
            ui.add_space(8.0);
            let time = match self.phase {
                Phase::Running { .. } => {
                    let el = p * REAL_SECONDS;
                    format!("{} {} · {} {}", clock(el), t.elapsed, t.left, clock(REAL_SECONDS - el))
                }
                Phase::Done { .. } => format!("{REAL_SECONDS:.1} s"),
                _ => String::new(),
            };
            ui.label(RichText::new(time).monospace().color(MUTED));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  {}", ph::LIST, t.log)).on_hover_text(t.log_tip).clicked() {
                    self.screen = Screen::Settings;
                    self.settings_tab = 2;
                }
            });
        });
    }

    fn export_modal(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let resp = egui::Modal::new(Id::new("export")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(t.export).strong().size(15.0));
            ui.separator();
            for (i, label) in t.export_targets.iter().enumerate() {
                ui.checkbox(&mut self.targets[i], *label);
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.format).strong());
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 9.0) / 4.0;
                for (i, name) in ["PNG", "JPEG", "WebP", "PSD"].iter().enumerate() {
                    if ui.add_sized([w, 28.0], egui::Button::new(*name).selected(self.format == i)).clicked() {
                        self.format = i;
                    }
                }
            });
            ui.add_space(10.0);
            ui.spacing_mut().slider_width = 360.0;
            match self.format {
                0 => {
                    ui.label(RichText::new(t.png_level).strong());
                    ui.add(egui::Slider::new(&mut self.png_level, 0..=9));
                }
                1 | 2 => {
                    ui.label(RichText::new(t.quality).strong());
                    ui.add(egui::Slider::new(&mut self.quality, 1..=100));
                }
                _ => {}
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.out_dir).strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.out_dir).desired_width(ui.available_width() - 70.0).font(egui::TextStyle::Monospace));
                if ui.button(t.browse).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.out_dir = dir.display().to_string();
                    }
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = egui::Button::new(RichText::new(t.do_export).strong().color(BLUE)).fill(SEL).stroke(Stroke::new(1.0, BLUE));
                if ui.add(go).clicked() {
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
    }

    // ---- settings screen ---------------------------------------------------------------------------

    fn settings_toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal_centered(|ui| {
            if ui.button(format!("{}  {}", ph::ARROW_LEFT, t.back)).on_hover_text(t.back_tip).clicked() {
                self.screen = Screen::Main;
            }
            ui.label(RichText::new(t.settings).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.language_combo(ui));
        });
    }

    fn settings_body(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        match self.settings_tab {
            0 => self.models_tab(ui, t),
            1 => self.comfy_tab(ui, t),
            _ => self.log_tab(ui, t),
        }
    }

    fn models_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let kinds: [usize; 4] = if self.rechecked { [0, 0, 0, 0] } else { [0, 1, 2, 3] };
        let ok = kinds.iter().filter(|&&k| k == 0).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.models).strong().size(15.0));
            ui.label(RichText::new(format!("{ok}/4 {}", t.match_word)).color(if ok == 4 { GREEN } else { AMBER }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(format!("{}  {}", ph::ARROWS_CLOCKWISE, t.recheck)).color(BLUE))
                    .fill(SEL)
                    .stroke(Stroke::new(1.0, BLUE));
                if ui.add(b).on_hover_text(t.recheck_tip).clicked() {
                    self.rechecked = true;
                }
            });
        });
        ui.add_space(6.0);
        Frame::new()
            .fill(Color32::from_rgb(0xff, 0xf8, 0xe5))
            .stroke(Stroke::new(1.0, Color32::from_rgb(0xe6, 0xd7, 0xa8)))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(ph::INFO).color(Color32::from_rgb(0x5c, 0x4a, 0x14)));
                    ui.label(RichText::new(t.no_download).color(Color32::from_rgb(0x5c, 0x4a, 0x14)));
                });
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized([110.0, 24.0], egui::Label::new(t.model_folder));
            ui.add(egui::TextEdit::singleline(&mut self.model_dir).desired_width(ui.available_width() - 70.0).font(egui::TextStyle::Monospace));
            let _ = ui.button(t.open).on_hover_text(t.open_folder_tip);
        });
        ui.add_space(8.0);
        let files = [
            ("qwen_image_2.1_int8_convrot.safetensors", "diffusion_models", "7.26 GB"),
            ("qwen3vl_8b_int8_convrot.safetensors", "text_encoders", "9.35 GB"),
            ("qwen_image_2.1_vae_bf16.safetensors", "vae", "0.68 GB"),
            ("birefnet.safetensors", "background_removal", "0.44 GB"),
        ];
        Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("models").striped(true).num_columns(6).spacing([14.0, 8.0]).show(ui, |ui| {
                for h in ["", t.col_file, t.col_dir, t.col_size, t.col_hash, t.col_source] {
                    ui.label(RichText::new(h).color(MUTED));
                }
                ui.end_row();
                for (i, (file, dir, size)) in files.iter().enumerate() {
                    let (icon, color) = match kinds[i] {
                        0 => (ph::CHECK_CIRCLE, GREEN),
                        1 => (ph::CIRCLE_NOTCH, BLUE),
                        2 => (ph::CIRCLE_DASHED, AMBER),
                        _ => (ph::X_CIRCLE, RED),
                    };
                    ui.label(RichText::new(icon).size(17.0).color(color));
                    // Grid columns keep the width of their first layout, so cells must not wrap.
                    ui.vertical(|ui| {
                        ui.set_min_width(300.0);
                        ui.add(egui::Label::new(t.purposes[i]).extend());
                        ui.add(egui::Label::new(RichText::new(*file).monospace().small().color(MUTED)).extend());
                    });
                    ui.add(egui::Label::new(RichText::new(*dir).monospace().color(MUTED)).extend());
                    ui.add(egui::Label::new(RichText::new(*size).monospace()).extend());
                    ui.add(egui::Label::new(RichText::new(t.states[kinds[i]]).color(color)).extend());
                    let _ = ui.button(format!("{}  {}", ph::LINK, t.source_page));
                    ui.end_row();
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.label(RichText::new(t.only_safetensors).color(MUTED));
            ui.add_space(12.0);
            ui.label(RichText::new(t.total).color(MUTED));
        });
    }

    fn comfy_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        ui.label(RichText::new("ComfyUI").strong().size(15.0));
        ui.label(RichText::new(t.comfy_install).color(MUTED));
        ui.add_space(8.0);
        egui::Grid::new("comfy").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label(t.location);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.comfy_dir).desired_width(520.0).font(egui::TextStyle::Monospace));
                if ui.button(t.browse).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.comfy_dir = dir.display().to_string();
                    }
                }
            });
            ui.end_row();
            ui.label(t.state);
            ui.label(RichText::new(format!("●  {}", t.comfy_state)).color(GREEN));
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(t.gpu_memory).strong());
        ui.checkbox(&mut self.keep_loaded, t.keep_loaded);
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.add(egui::Label::new(RichText::new(t.keep_loaded_desc).color(MUTED)).wrap());
        });
    }

    fn log_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let lines = [
            ("12:40:02", "INFO", r"App started; launching ComfyUI from D:\ComfyUI_windows_portable"),
            ("12:40:11", "INFO", "ComfyUI ready (v0.40.0, port 8188)"),
            ("12:40:11", "INFO", "Model check: 4/4 match"),
            ("12:41:05", "INFO", "Input VRChat_2026-10-15.png 2160x3840; model resolution 1536"),
            ("12:41:05", "INFO", "Shading started: light=top-left strength=1.0 scope=subject seed=random"),
            ("12:41:12", "INFO", "AI: first step after 6.9 s"),
            ("12:42:03", "INFO", "AI: 25/25 steps done in 58.1 s"),
            ("12:42:05", "INFO", "Subject mask (BiRefNet) in 1.6 s"),
            ("12:42:08", "WARN", "5.3% of pixels got brighter; they are not included in the shadow layer"),
            ("12:42:09", "INFO", "Shadow layer 1152x2048 -> 2160x3840 in 3.2 s"),
            ("12:42:10", "INFO", "Done in 64.8 s"),
            ("12:45:31", "ERROR", "Export failed: cannot write to the output folder (access denied)"),
        ];
        let filter = [None, Some("INFO"), Some("WARN"), Some("ERROR")][self.log_level];
        let shown: Vec<_> = lines.iter().filter(|(_, lv, _)| filter.is_none_or(|f| f == *lv)).collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.log).strong().size(15.0));
            ui.add_space(8.0);
            for (i, name) in t.levels.iter().enumerate() {
                if ui.add(egui::Button::new(*name).selected(self.log_level == i)).clicked() {
                    self.log_level = i;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let _ = ui.button(t.open_log_folder);
                if ui.button(t.copy).on_hover_text(t.copy_tip).clicked() {
                    let text = shown.iter().map(|(a, b, c)| format!("{a}  {b:<5} {c}")).collect::<Vec<_>>().join("\n");
                    ui.ctx().copy_text(text);
                }
            });
        });
        ui.add_space(6.0);
        Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (time, level, text) in &shown {
                    let color = match *level {
                        "WARN" => AMBER,
                        "ERROR" => RED,
                        _ => TEXT,
                    };
                    ui.label(RichText::new(format!("{time}  {level:<5}  {text}")).monospace().color(color));
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(path) = dropped.first() {
            self.open_path(&ctx, path);
        }
        self.tick(&ctx);
        self.handle_shot(&ctx);

        let bar = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(6, 4)).stroke(Stroke::new(1.0, LINE));
        let side = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, LINE));
        match self.screen {
            Screen::Main => {
                egui::Panel::top("toolbar").frame(bar).exact_size(36.0).show(ui, |ui| self.toolbar(ui));
                egui::Panel::bottom("status").frame(bar).exact_size(40.0).show(ui, |ui| self.status_bar(ui));
                egui::Panel::left("controls").frame(side).exact_size(270.0).resizable(false).show(ui, |ui| self.controls(ui));
                egui::Panel::right("history")
                    .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(6)).stroke(Stroke::new(1.0, LINE_DARK)))
                    .exact_size(280.0)
                    .resizable(false)
                    .show_collapsible(ui, &mut self.history_open.clone(), |ui| self.history(ui));
                egui::CentralPanel::default().frame(Frame::new().fill(CANVAS)).show(ui, |ui| self.canvas(ui));
                if self.export_open {
                    self.export_modal(&ctx);
                }
            }
            Screen::Settings => {
                egui::Panel::top("settings_toolbar").frame(bar).exact_size(36.0).show(ui, |ui| self.settings_toolbar(ui));
                egui::Panel::left("settings_nav")
                    .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(8)).stroke(Stroke::new(1.0, LINE)))
                    .exact_size(180.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        let t = self.t();
                        for (i, name) in t.tabs.iter().enumerate() {
                            if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(*name).selected(self.settings_tab == i)).clicked() {
                                self.settings_tab = i;
                            }
                        }
                    });
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(BG).inner_margin(Margin::same(12)))
                    .show(ui, |ui| self.settings_body(ui));
            }
        }
    }
}

/// Button with an icon glyph followed by a label.
struct Button;
impl Button {
    fn icon_text(icon: &str, text: &str) -> egui::Button<'static> {
        egui::Button::new(format!("{icon}  {text}"))
    }
}

fn clock(sec: f64) -> String {
    let s = sec.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn dashed_rect(painter: &egui::Painter, r: Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        painter.extend(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 5.0, 4.0));
    }
}

fn checkerboard(painter: &egui::Painter, r: Rect) {
    painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
    let s = 12.0;
    let (mut y, mut row) = (r.top(), 0);
    while y < r.bottom() {
        let mut x = r.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < r.right() {
            let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(s)).intersect(r);
            painter.rect_filled(cell, CornerRadius::ZERO, Color32::from_gray(0xd6));
            x += s * 2.0;
        }
        y += s;
        row += 1;
    }
}

/// Phosphor icons plus a Japanese system font (not bundled, keeps the exe small).
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    for path in [r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\meiryo.ttc", r"C:\Windows\Fonts\msgothic.ttc"] {
        if let Ok(bytes) = std::fs::read(path) {
            // Yu Gothic / Meiryo sit high in egui's line box; nudge the glyphs down to line up with icons and checkboxes.
            let tweak = egui::FontTweak { y_offset_factor: 0.24, ..Default::default() };
            fonts.font_data.insert("jp".into(), Arc::new(egui::FontData::from_owned(bytes).tweak(tweak)));
            // Proportional text uses the Japanese font first so Latin and Japanese share one baseline;
            // monospace keeps the default (Hack) and falls back to it for Japanese.
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "jp".into());
            fonts.families.entry(egui::FontFamily::Monospace).or_default().push("jp".into());
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// Light, flat, small-radius look.
fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(8.0, 4.0);
        s.spacing.interact_size.y = 26.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(12.0));
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = BG;
        v.selection.bg_fill = SEL;
        v.selection.stroke = Stroke::new(1.0, BLUE);
        v.override_text_color = Some(TEXT);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
            w.corner_radius = CornerRadius::same(2);
        }
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(0xe2);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.hovered.weak_bg_fill = Color32::from_gray(0xd8);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x9a));
        v.widgets.active.weak_bg_fill = Color32::from_gray(0xcc);
        v.window_corner_radius = CornerRadius::same(2);
        v.menu_corner_radius = CornerRadius::same(2);
    });
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Atozuke Kage MOD")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Atozuke Kage MOD", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
