//! Textures of the shown result. Flattening runs off the UI thread; the canvas keeps the last finished image until
//! the next one arrives, so dragging a slider stays smooth. Exports always flatten at full resolution.

use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Color32, TextureFilter, TextureHandle, TextureOptions};
use kage::blend::{self, View};
use kage::pipeline::Thumb;
use kage::resample::{self, Filter};

use super::App;

/// What a finished stack texture shows.
#[derive(PartialEq, Clone, Copy)]
pub(super) struct StackKey {
    serial: u64,
    view: View,
    nearest: bool,
}

pub(super) struct Views {
    pub(super) id: u64,
    /// The visible layers flattened: what the canvas shows.
    pub(super) stack: TextureHandle,
    pub(super) shown: Option<StackKey>,
    job: Option<(StackKey, Receiver<egui::ColorImage>)>,
}

impl Views {
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
}

/// Canvas sampling: mipmaps so reduced views use every pixel; single pixels stay square when `nearest`.
pub(super) fn display_options(nearest: bool) -> TextureOptions {
    TextureOptions {
        magnification: if nearest { TextureFilter::Nearest } else { TextureFilter::Linear },
        mipmap_mode: Some(TextureFilter::Linear),
        ..TextureOptions::LINEAR
    }
}

pub(super) fn max_side(ctx: &egui::Context) -> usize {
    ctx.input(|i| i.max_texture_side).max(2048)
}

/// RGBA texture data, downscaled when the GPU cannot hold the full size (display only).
pub(super) fn color_image(rgba: &[u8], w: usize, h: usize, max: usize) -> egui::ColorImage {
    if w.max(h) <= max {
        return egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
    }
    let k = max as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1));
    egui::ColorImage::from_rgba_unmultiplied([nw, nh], &resample::resize_rgba(rgba, w, h, nw, nh, Filter::Bilinear))
}

pub(super) fn thumb_texture(ctx: &egui::Context, thumb: &Thumb) -> TextureHandle {
    ctx.load_texture("thumb", egui::ColorImage::from_rgba_unmultiplied([thumb.w, thumb.h], &thumb.rgba), TextureOptions::LINEAR)
}

impl App {
    pub(super) fn ensure_views(&mut self, ctx: &egui::Context) {
        self.sync_input_filter(ctx);
        let Some(id) = self.current.filter(|_| self.entry(self.current).is_some()) else {
            self.views = None;
            return;
        };
        if self.views.as_ref().map(|v| v.id) != Some(id) {
            let stack = ctx.load_texture("stack", egui::ColorImage::filled([1, 1], Color32::TRANSPARENT), TextureOptions::LINEAR);
            self.views = Some(Views { id, stack, shown: None, job: None });
        }
        let view = self.view();
        let nearest = self.nearest;
        let max = max_side(ctx);
        let e = self.history.iter().find(|e| e.id == id).expect("entry");
        let key = StackKey { serial: e.serial, view, nearest };
        let views = self.views.as_mut().expect("views");
        if let Some((done, rx)) = &views.job {
            if let Ok(image) = rx.try_recv() {
                views.stack.set(image, display_options(done.nearest));
                views.shown = Some(*done);
                views.job = None;
            }
        }
        if views.job.is_none() && views.shown != Some(key) {
            let (original, ai_full, layers) = (e.result.original.clone(), e.result.ai_full.clone(), e.result.solved.layers.clone());
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let rgba = blend::flatten_view(&original, &ai_full, &layers, &view);
                let _ = tx.send(color_image(&rgba, layers.w, layers.h, max));
                repaint.request_repaint();
            });
            views.job = Some((key, rx));
        }
    }

    /// The opened image's texture follows the same sampling as the stack.
    fn sync_input_filter(&mut self, ctx: &egui::Context) {
        let Some(img) = &mut self.image else { return };
        let wanted = (img.tex.id(), self.nearest);
        if self.input_filter == Some(wanted) {
            return;
        }
        let changed = self.input_filter.is_some_and(|(id, _)| id == wanted.0);
        if changed || self.nearest {
            img.tex.set(color_image(&img.img.rgba, img.img.w, img.img.h, max_side(ctx)), display_options(self.nearest));
        }
        self.input_filter = Some(wanted);
    }
}
