//! Atozuke Kage MOD — adds AI shading to an image as a separate shadow layer (egui front end).
//!
//! The work happens in the `kage` library: a private ComfyUI instance draws the shading (Qwen-Image 2.1), BiRefNet
//! finds the subject, and the shadow layer is solved locally.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod i18n;
mod theme;

use std::io::Write;
use std::path::Path;

use eframe::egui;
use kage::config::{self, Config};

const DEFAULT_SIZE: [f32; 2] = [1440.0, 900.0];
const MIN_SIZE: [f32; 2] = [960.0, 640.0];

/// A panic ends the process (panic = "abort") and the window subsystem has no stderr: keep the reason in the log.
fn log_panics(data: &Path) {
    let path = data.join("logs").join("kage.log");
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "PANIC  {info}");
        }
    }));
}

/// KAGE_W / KAGE_H (layout checks), else the size of the last session.
fn window_size(data: &Path) -> [f32; 2] {
    let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse().ok());
    if let (Some(w), Some(h)) = (env("KAGE_W"), env("KAGE_H")) {
        return [w, h];
    }
    let [w, h] = Config::load(data).0.window.unwrap_or(DEFAULT_SIZE);
    [w.max(MIN_SIZE[0]), h.max(MIN_SIZE[1])]
}

fn main() -> eframe::Result {
    let data = config::data_dir();
    log_panics(&data);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("後付け影MOD")
            .with_inner_size(window_size(&data))
            .with_min_inner_size(MIN_SIZE)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("embedded icon"))
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native("後付け影MOD", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))));
    if let Err(e) = &result {
        rfd::MessageDialog::new().set_title("後付け影MOD").set_description(e.to_string()).set_level(rfd::MessageLevel::Error).show();
    }
    result
}
