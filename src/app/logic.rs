//! What happens behind the UI: opening images, runs, re-solving, exporting, saving the settings.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Vec2};
use kage::blend::SHADOW;
use kage::comfy::{self, Failure};
use kage::export::{self, Encoding, Job, FORMATS};
use kage::pipeline::{self, Event, Params, Request};
use kage::shading::Image8;

use super::views::{color_image, display_options, max_side, thumb_texture};
use super::{App, Entry, Input, Phase, Resolve, Toast};
use crate::i18n::T;

/// Seconds after the last slider change before the shadow layer is re-solved.
const RESOLVE_DELAY: f64 = 0.35;
/// Seconds a success message stays; failures stay until closed.
const TOAST_SECONDS: f64 = 6.0;
const HISTORY_LIMIT: usize = 30;
/// A given shaded image this far off the original's aspect ratio gets a warning (model output rounds its size).
const ASPECT_TOLERANCE: f32 = 0.02;
const IMAGE_FILTER: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
/// Elapsed time in the status bar: no need for more frames than this during a long run.
const RUN_REPAINT: std::time::Duration = std::time::Duration::from_millis(250);

/// The message for a failed run, in the UI's language; the raw text goes to the log.
pub(super) fn failure_text(t: &'static T, f: &Failure) -> &'static str {
    match f {
        Failure::NotSetUp => t.fail_not_set_up,
        Failure::TooOld(_) => t.fail_too_old,
        Failure::NotRunning(_) => t.fail_not_running,
        Failure::ModelsInvalid => t.fail_models,
        Failure::Stopped(_) => t.fail_stopped,
        Failure::Restarted => t.fail_restarted,
        Failure::OutOfMemory(_) => t.fail_memory,
        Failure::Other(_) => t.fail_other,
    }
}

impl App {
    pub(super) fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        match Image8::load(path) {
            Ok(img) => {
                let tex = ctx.load_texture("input", color_image(&img.rgba, img.w, img.h, max_side(ctx)), display_options(false));
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.log.info(format!("Opened {name} {}x{}", img.w, img.h));
                if img.deep {
                    self.log.warn(format!("{name}: converted to 8 bits per channel"));
                    self.warn(ctx, self.t().deep_input);
                }
                self.image = Some(Input { img: Arc::new(img), tex, name });
                self.input_filter = None;
                self.zoom = None;
                self.pan = Vec2::ZERO;
                self.current = None;
                if !self.running() {
                    self.phase = Phase::Idle;
                }
            }
            Err(e) => {
                self.log.error(format!("Could not open {}: {e}", path.display()));
                self.warn(ctx, &e);
            }
        }
    }

    pub(super) fn warn(&mut self, ctx: &egui::Context, text: &str) {
        self.toast = Some(Toast { ok: false, text: text.into(), at: ctx.input(|i| i.time), folder: None });
    }

    pub(super) fn pick_image(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Images", &IMAGE_FILTER).pick_file() {
            self.open_path(ctx, &path);
        }
    }

    /// Extract the shadows from a shaded version of the open image.
    pub(super) fn pick_shaded(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().set_title(self.t().pick_shaded).add_filter("Images", &IMAGE_FILTER).pick_file() else { return };
        match Image8::load(&path) {
            Ok(img) => {
                if let Some(input) = &self.image {
                    let ratio = |w: usize, h: usize| w as f32 / h as f32;
                    if (ratio(img.w, img.h) / ratio(input.img.w, input.img.h) - 1.0).abs() > ASPECT_TOLERANCE {
                        self.log.warn(format!("Shaded image {}x{} has another aspect ratio than {}x{}", img.w, img.h, input.img.w, input.img.h));
                        self.warn(ctx, self.t().aspect_mismatch);
                    }
                }
                self.start_run(ctx, Some(Arc::new(img)));
            }
            Err(e) => self.warn(ctx, &e),
        }
    }

    fn params(&self, seed: u64) -> Params {
        Params {
            light_dir: self.cfg.light_dir,
            extra: self.cfg.extra.clone(),
            strength: self.cfg.strength,
            background: self.cfg.bg_shadow,
            gray: self.cfg.gray,
            ghost: self.cfg.ghost,
            seed,
        }
    }

    /// Models verified and ComfyUI usable (running or starting).
    pub(super) fn ready(&self) -> bool {
        self.verifier.all_ok() && matches!(self.comfy.state(), comfy::State::Ready { .. } | comfy::State::Starting)
    }

    fn spawn_run(&mut self, ctx: &egui::Context, input: Input, params: Params, shaded: Option<Arc<Image8>>) {
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        let repaint = ctx.clone();
        pipeline::spawn(
            Request {
                original: input.img.clone(),
                name: input.name.clone(),
                params,
                shaded,
                model_dir: PathBuf::from(self.cfg.model_dir.trim()),
                keep_loaded: self.cfg.keep_loaded,
            },
            pipeline::Ctx {
                comfy: self.comfy.clone(),
                verifier: self.verifier.clone(),
                log: self.log.clone(),
                cancel: self.cancel.clone(),
                tx,
                notify: Arc::new(move || repaint.request_repaint()),
            },
        );
        self.run_rx = Some(rx);
        self.run_input = Some(input);
        self.phase = Phase::Running { started: ctx.input(|i| i.time) };
        self.stage = 0;
        self.waiting_comfy = false;
        self.steps = (0, 0);
        self.first_step_at = None;
        self.error = None;
    }

    pub(super) fn start_run(&mut self, ctx: &egui::Context, shaded: Option<Arc<Image8>>) {
        let Some(input) = self.image.clone() else { return };
        // Keep random seeds below 2^53 so they survive any JSON round trip and are easy to copy.
        let seed = self.seed.trim().parse::<u64>().unwrap_or_else(|_| comfy::random_u64() >> 11);
        let params = self.params(seed);
        self.spawn_run(ctx, input, params, shaded);
    }

    pub(super) fn select_entry(&mut self, id: u64) {
        let Some(e) = self.history.iter().find(|e| e.id == id) else { return };
        let p = &e.result.params;
        self.cfg.light_dir = p.light_dir;
        self.cfg.extra = p.extra.clone();
        self.cfg.strength = p.strength;
        self.cfg.bg_shadow = p.background;
        self.cfg.gray = p.gray;
        self.cfg.ghost = p.ghost;
        self.post_seen = (p.strength, p.gray, p.ghost, p.background);
        self.image = Some(e.input.clone());
        self.input_filter = None;
        self.current = Some(id);
    }

    pub(super) fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let events: Vec<Event> = self.run_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for ev in events {
            match ev {
                Event::WaitingForComfy => self.waiting_comfy = true,
                Event::Stage(s) => {
                    self.waiting_comfy = false;
                    self.stage = s;
                }
                Event::Steps(v, m) => {
                    self.waiting_comfy = false;
                    if self.first_step_at.is_none() {
                        self.first_step_at = Some(now);
                    }
                    self.steps = (v, m);
                    self.last_step_at = now;
                }
                Event::Done(r) => self.finish_run(ctx, r),
                Event::Failed(failure) => {
                    self.error = Some(failure);
                    self.end_run(super::Phase::Failed);
                }
                Event::Cancelled => self.end_run(Phase::Stopped),
            }
        }
        if self.running() {
            ctx.request_repaint_after(RUN_REPAINT);
        }
        if self.dirs_pending && !self.running() {
            self.apply_dirs();
        }
        self.tick_resolve(ctx, now);
        self.tick_export(ctx, now);
        self.tick_config(now);
    }

    fn end_run(&mut self, phase: Phase) {
        self.phase = phase;
        self.run_rx = None;
        if self.mask_run.take().is_some() {
            // The mask could not be made: back to the state the shown result has.
            self.cfg.bg_shadow = true;
            self.post_seen.3 = true;
        }
    }

    fn finish_run(&mut self, ctx: &egui::Context, r: Box<pipeline::RunResult>) {
        self.phase = Phase::Done;
        self.run_rx = None;
        if let Some(id) = self.mask_run.take() {
            if let Some(e) = self.history.iter_mut().find(|e| e.id == id) {
                e.thumb = thumb_texture(ctx, &r.solved.shaded_thumb);
                e.layer_thumbs[SHADOW] = thumb_texture(ctx, &r.solved.shadow_thumb);
                e.result.mask = r.mask;
                e.result.solved = r.solved;
                e.result.params.background = false;
                e.serial += 1;
                e.exported = false;
            }
            return;
        }
        let input = self.run_input.take().unwrap_or_else(|| self.image.clone().expect("input"));
        let id = self.next_id;
        self.next_id += 1;
        let thumb = thumb_texture(ctx, &r.solved.shaded_thumb);
        let layer_thumbs = [thumb_texture(ctx, &r.solved.shadow_thumb), thumb_texture(ctx, &r.ai_thumb), thumb_texture(ctx, &r.original_thumb)];
        let shown = self.image.as_ref().is_some_and(|i| Arc::ptr_eq(&i.img, &input.img));
        self.history.insert(0, Entry { id, serial: 0, input, result: *r, thumb, layer_thumbs, exported: false });
        self.history.truncate(HISTORY_LIMIT);
        if shown {
            self.current = Some(id);
        } else {
            self.toast = Some(Toast { ok: true, text: self.t().added_to_history.into(), at: ctx.input(|i| i.time), folder: None });
        }
        self.visible = [true, false, true];
        self.solo = None;
        self.selected_layer = SHADOW;
    }

    /// The adjustments the shown result should have. The background can only be turned off with a subject mask.
    fn wanted(&self, e: &Entry) -> Params {
        let p = &e.result.params;
        let background = if self.cfg.bg_shadow || e.result.mask.is_some() { self.cfg.bg_shadow } else { p.background };
        Params { strength: self.cfg.strength, gray: self.cfg.gray, ghost: self.cfg.ghost, background, ..p.clone() }
    }

    /// The shown result already has what the controls say (nothing left to re-solve).
    fn settled(&self) -> bool {
        if self.resolve.is_some() || self.mask_run.is_some() {
            return false;
        }
        self.entry(self.current).is_none_or(|e| same_adjustments(&e.result.params, &self.wanted(e)))
    }

    /// Strength, gray, double lines and background re-solve the shown layer from the saved AI output (no new AI run),
    /// also while another image is generating.
    fn tick_resolve(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(r) = &self.resolve {
            if let Ok(solved) = r.rx.try_recv() {
                let (id, params) = (r.id, r.params.clone());
                self.resolve = None;
                if let Some(e) = self.history.iter_mut().find(|e| e.id == id) {
                    e.thumb = thumb_texture(ctx, &solved.shaded_thumb);
                    e.layer_thumbs[SHADOW] = thumb_texture(ctx, &solved.shadow_thumb);
                    e.result.solved = solved;
                    e.result.params = params;
                    e.serial += 1;
                    e.exported = false;
                }
            }
            return;
        }
        let seen = (self.cfg.strength, self.cfg.gray, self.cfg.ghost, self.cfg.bg_shadow);
        if seen != self.post_seen {
            self.post_seen = seen;
            self.post_changed_at = now;
        }
        let Some(e) = self.entry(self.current) else { return };
        let wanted = self.wanted(e);
        let needs_mask = !self.cfg.bg_shadow && e.result.mask.is_none();
        if same_adjustments(&e.result.params, &wanted) && !needs_mask {
            return;
        }
        if now - self.post_changed_at < RESOLVE_DELAY {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(RESOLVE_DELAY));
            return;
        }
        if needs_mask {
            if self.mask_run.is_none() && !self.running() && self.ready() {
                self.start_mask_run(ctx);
            }
            if same_adjustments(&self.entry(self.current).expect("entry").result.params, &wanted) {
                return;
            }
        }
        let Some(e) = self.entry(self.current) else { return };
        let (original, shaded, mask, log) = (e.result.original.clone(), e.result.shaded.clone(), e.result.mask.clone(), self.log.clone());
        let id = e.id;
        let (tx, rx) = mpsc::channel();
        let p2 = wanted.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let solved = pipeline::solve(&original, &shaded, mask.as_deref(), &p2, &log);
            let _ = tx.send(solved);
            repaint.request_repaint();
        });
        self.resolve = Some(Resolve { id, params: wanted, rx });
    }

    /// Turning the background off needs the subject mask, which a run with the background on did not make. The AI
    /// output stays the same: only BiRefNet runs.
    fn start_mask_run(&mut self, ctx: &egui::Context) {
        let Some(e) = self.entry(self.current) else { return };
        let params = Params { background: false, ..self.wanted(e) };
        let (input, shaded, id) = (e.input.clone(), e.result.shaded.clone(), e.id);
        self.spawn_run(ctx, input, params, Some(shaded));
        self.mask_run = Some(id);
    }

    fn tick_config(&mut self, now: f64) {
        self.cfg.lang = self.lang_code().into();
        if self.cfg != self.saved_cfg {
            if self.cfg_changed_at == 0.0 {
                self.cfg_changed_at = now;
            } else if now - self.cfg_changed_at > 1.0 {
                self.save_config();
            }
        }
    }

    pub(super) fn save_config(&mut self) {
        if let Err(e) = self.cfg.save(&self.data) {
            self.log.error(format!("Could not save the settings: {e}"));
        }
        self.saved_cfg = self.cfg.clone();
        self.cfg_changed_at = 0.0;
    }

    /// Apply changed model / ComfyUI directories: re-verify and restart ComfyUI (it reads the model paths at start).
    /// A running job keeps its instance; the change waits until the job ends.
    pub(super) fn apply_dirs(&mut self) {
        let dirs = (self.cfg.model_dir.trim().to_string(), self.cfg.comfy_dir.trim().to_string());
        if dirs == self.applied_dirs {
            self.dirs_pending = false;
            return;
        }
        if self.running() {
            self.dirs_pending = true;
            return;
        }
        self.dirs_pending = false;
        if dirs.0 != self.applied_dirs.0 {
            self.verifier.start(PathBuf::from(&dirs.0));
        }
        self.comfy.start(&dirs.1, Path::new(&dirs.0));
        self.applied_dirs = dirs;
    }

    /// Export the shown result as the controls show it: waits for a pending re-solve.
    pub(super) fn request_export(&mut self, ctx: &egui::Context) {
        if self.settled() {
            self.export(ctx);
        } else {
            self.export_pending = true;
        }
    }

    fn export(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let Some(e) = self.entry(self.current) else { return };
        let targets = self.cfg.export_targets;
        let format = FORMATS[self.cfg.export_format];
        if !targets.iter().any(|&x| x) {
            self.warn(ctx, t.nothing_to_export);
            return;
        }
        let r = &e.result;
        let (id, original, shaded, ai_full, layers) = (e.id, r.original.clone(), r.shaded.clone(), r.ai_full.clone(), r.solved.layers.clone());
        let view = self.view();
        let encoding = Encoding { format, png_level: self.cfg.png_level, quality: self.cfg.quality };
        let dir = PathBuf::from(self.cfg.out_dir.trim());
        let stem = Path::new(&r.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
        let log = self.log.clone();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let outcome = export::run_job(&Job {
                original: &original,
                shaded: &shaded,
                ai_full: &ai_full,
                layers: &layers,
                view,
                targets,
                encoding,
                dir: &dir,
                stem: &stem,
            });
            for p in &outcome.written {
                log.info(format!("Exported {}", p.display()));
            }
            if let Some(e) = &outcome.error {
                log.error(format!("Export failed: {e}"));
            }
            let _ = tx.send((id, outcome));
            repaint.request_repaint();
        });
        self.export_rx = Some(rx);
    }

    fn tick_export(&mut self, ctx: &egui::Context, now: f64) {
        let t = self.t();
        if self.export_pending && self.settled() {
            self.export_pending = false;
            self.export(ctx);
        }
        if let Some(rx) = &self.export_rx {
            if let Ok((id, outcome)) = rx.try_recv() {
                self.export_rx = None;
                let n = outcome.written.len();
                let dir = outcome.written.first().and_then(|p| p.parent()).map(Path::to_path_buf);
                self.toast = Some(match outcome.error {
                    None => {
                        if let Some(e) = self.history.iter_mut().find(|e| e.id == id) {
                            e.exported = true;
                        }
                        self.last_export_dir = dir.clone();
                        Toast { ok: true, text: t.exported.replace("{}", &n.to_string()), at: now, folder: dir }
                    }
                    Some(e) if n > 0 => Toast { ok: false, text: format!("{}: {e}", t.export_partial.replace("{}", &n.to_string())), at: now, folder: dir },
                    Some(e) => Toast { ok: false, text: format!("{}: {e}", t.export_failed), at: now, folder: None },
                });
            }
        }
        if let Some(toast) = &self.toast {
            if toast.ok && now - toast.at > TOAST_SECONDS {
                self.toast = None;
            } else if toast.ok {
                ctx.request_repaint_after(RUN_REPAINT);
            }
        }
    }

    pub(super) fn stop_run(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        self.peek_original = false;
        if ctx.egui_wants_keyboard_input() || self.export_open || self.quit_open {
            return;
        }
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key)));
        if pressed(Key::O) {
            self.pick_image(ctx);
        }
        if pressed(Key::S) && self.current.is_some() {
            self.export_open = true;
        }
        if pressed(Key::Num0) {
            self.zoom_fit();
        }
        if pressed(Key::Num1) {
            self.zoom_actual(ctx);
        }
        if pressed(Key::Plus) || pressed(Key::Equals) || pressed(Key::Semicolon) {
            self.zoom_by(super::canvas::ZOOM_STEP);
        }
        if pressed(Key::Minus) {
            self.zoom_by(1.0 / super::canvas::ZOOM_STEP);
        }
        self.peek_original = ctx.input(|i| i.key_down(Key::C));
    }
}

fn same_adjustments(a: &Params, b: &Params) -> bool {
    (a.strength, a.gray, a.ghost, a.background) == (b.strength, b.gray, b.ghost, b.background)
}
