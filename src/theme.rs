//! Look of the app: colors, type sizes, fonts and the egui style. Light, flat, radius 2.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Shadow, Stroke, Vec2};

pub const BLUE: Color32 = Color32::from_rgb(0x00, 0x53, 0x7d);
pub const SEL: Color32 = Color32::from_rgb(0xbf, 0xe3, 0xff);
pub const PANEL: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
pub const BG: Color32 = Color32::from_rgb(0xf8, 0xf8, 0xf8);
pub const LINE: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
pub const LINE_DARK: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
pub const TEXT: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
pub const MUTED: Color32 = Color32::from_rgb(0x5e, 0x5e, 0x5e);
pub const GREEN: Color32 = Color32::from_rgb(0x1f, 0x7a, 0x4a);
pub const AMBER: Color32 = Color32::from_rgb(0x8a, 0x5a, 0x08);
pub const RED: Color32 = Color32::from_rgb(0xb0, 0x3a, 0x2e);
/// Icons that show a state (hidden layer, idle).
pub const STATE_OFF: Color32 = Color32::from_gray(0x7a);
/// Thin marks over the panel background: checkbox frames, slider rails.
pub const CONTROL_EDGE: Color32 = Color32::from_gray(0x8a);
/// Fill of a hovered row.
pub const HOVER: Color32 = Color32::from_gray(0xe6);
/// Fill of a small icon button inside a row.
pub const SLOT: Color32 = Color32::from_gray(0xf8);
/// Canvas surround: light, mid gray, dark.
pub const CANVAS_SHADES: [Color32; 3] = [Color32::from_gray(0xe3), Color32::from_gray(0x80), Color32::from_gray(0x3a)];

/// Buttons, fields and combo boxes, whatever is next to each other on a line.
pub const CONTROL_HEIGHT: f32 = 28.0;
/// Generate and Export.
pub const ACTION_HEIGHT: f32 = 48.0;
/// Top bar, status bar and the view tools.
pub const BAR_HEIGHT: f32 = 36.0;
/// Number fields next to a slider.
pub const VALUE_WIDTH: f32 = 66.0;
/// From the window and panel edges to their content.
pub const PADDING_I: i8 = 10;
/// Between items of a group.
pub const GAP: f32 = 6.0;
/// Between groups.
pub const SECTION_GAP: f32 = 12.0;

pub const SIZE_SMALL: f32 = 12.0;
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_TITLE: f32 = 16.0;
pub const SIZE_ICON: f32 = 16.0;

const RADIUS: u8 = 2;
/// Radians per second.
const SPINNER_SPEED: f64 = 5.0;
/// Length of the arc in radians.
const SPINNER_ARC: f32 = 4.5;
const SPINNER_POINTS: usize = 24;
const SPINNER_REPAINT: std::time::Duration = std::time::Duration::from_millis(33);

pub fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

/// The main action of a screen: a solid fill, unlike selection (SEL). Disabled it is a plain button.
pub fn primary_button(text: impl Into<String>, size: f32, enabled: bool) -> egui::Button<'static> {
    let text = RichText::new(text.into()).size(size);
    if enabled {
        egui::Button::new(text.strong().color(Color32::WHITE)).fill(BLUE).stroke(Stroke::new(1.0, BLUE))
    } else {
        egui::Button::new(text)
    }
}

/// A one-line text field of the common height.
pub fn text_field(text: &mut String, width: f32) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).desired_width(width).min_size(Vec2::new(width, CONTROL_HEIGHT)).vertical_align(egui::Align::Center)
}

/// A square button with only an icon, as high as the fields next to it.
pub fn icon_button(ui: &mut egui::Ui, icon: &str) -> egui::Response {
    ui.scope(|ui| {
        // The icon's line height plus the usual padding would make it taller.
        ui.spacing_mut().button_padding = Vec2::ZERO;
        ui.add_sized([CONTROL_HEIGHT, CONTROL_HEIGHT], egui::Button::new(RichText::new(icon).size(SIZE_ICON)))
    })
    .inner
}

/// A turning arc for work in progress, as large as an icon. It repaints slowly: a run has the GPU busy with the AI.
pub fn spinner(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(SIZE_ICON), egui::Sense::hover());
    let start = (ui.input(|i| i.time) * SPINNER_SPEED).rem_euclid(std::f64::consts::TAU) as f32;
    let r = rect.width() / 2.0 - 2.0;
    let points = (0..=SPINNER_POINTS).map(|i| rect.center() + r * Vec2::angled(start + i as f32 / SPINNER_POINTS as f32 * SPINNER_ARC)).collect();
    ui.painter().add(egui::Shape::line(points, Stroke::new(1.5, color)));
    ui.ctx().request_repaint_after(SPINNER_REPAINT);
}

/// The frame of a dialog.
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::new().fill(BG).stroke(Stroke::new(1.0, LINE_DARK)).corner_radius(radius()).inner_margin(egui::Margin::same(PADDING_I))
}

pub fn checkerboard(painter: &egui::Painter, r: Rect) {
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
pub fn setup_fonts(ctx: &egui::Context) {
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
pub fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        s.spacing.item_spacing = Vec2::splat(GAP);
        s.spacing.button_padding = Vec2::new(8.0, 2.0);
        s.spacing.interact_size.y = CONTROL_HEIGHT;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(SIZE_BODY));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(SIZE_BODY));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(SIZE_SMALL));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(SIZE_SMALL));
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = BG;
        v.selection.bg_fill = SEL;
        v.selection.stroke = Stroke::new(1.0, BLUE);
        v.override_text_color = Some(TEXT);
        v.slider_trailing_fill = true;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.window_stroke = Stroke::new(1.0, LINE_DARK);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
            w.corner_radius = radius();
        }
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(0xe2);
        v.widgets.inactive.bg_fill = Color32::from_gray(0xd0);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, CONTROL_EDGE);
        v.widgets.hovered.weak_bg_fill = Color32::from_gray(0xd8);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x70));
        v.widgets.active.weak_bg_fill = Color32::from_gray(0xcc);
        v.window_corner_radius = radius();
        v.menu_corner_radius = radius();
    });
}
