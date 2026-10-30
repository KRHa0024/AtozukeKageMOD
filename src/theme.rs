//! Look of the app: colors, type sizes, fonts and the egui style. Light, flat, radius 2.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Shadow, Stroke, Vec2};

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
/// Drop-zone outline.
pub const DASH: Color32 = Color32::from_gray(0x8a);
/// Canvas surround: light, mid gray, dark.
pub const CANVAS_SHADES: [Color32; 3] = [Color32::from_gray(0xe3), Color32::from_gray(0x80), Color32::from_gray(0x3a)];

pub const SIZE_SMALL: f32 = 12.0;
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_TITLE: f32 = 16.0;
pub const SIZE_ICON: f32 = 16.0;

const RADIUS: u8 = 2;

pub fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

pub fn dashed_rect(painter: &egui::Painter, r: Rect) {
    let stroke = Stroke::new(1.0, DASH);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        painter.extend(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 5.0, 4.0));
    }
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
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(8.0, 4.0);
        s.spacing.interact_size.y = 26.0;
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
