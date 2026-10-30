//! Atozuke Kage MOD — adds AI shading to an image as a separate shadow layer (egui front end).
//!
//! The work happens in the `kage` library: a private ComfyUI instance draws the shading (Qwen-Image 2.1), BiRefNet
//! finds the subject, and the shadow layer is solved locally.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod i18n;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextureHandle, Vec2,
};
use egui_phosphor::regular as ph;
use i18n::{t, Lang, T};
use kage::blend::{self, MENU_ORDER, MODES};
use kage::comfy::{self, Comfy};
use kage::config::{self, Config};
use kage::export::{self, Format, Pixels, PsdLayer, FORMATS};
use kage::log::{Level, Log};
use kage::models::{self, Status, Verifier, MODELS};
use kage::pipeline::{self, Event, Params, Request, RunResult};
use kage::resample::{self, Filter};
use kage::shading::{Image8, Layers};

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

/// Progress fraction where the AI stage ends; the rest is subject mask, shadow layer and finishing.
const AI_SHARE: f64 = 0.85;
/// Rough seconds after the last step (decode, subject mask, shadow layer) for the remaining-time estimate.
const AFTER_STEPS_SECONDS: f64 = 6.0;
/// Seconds after the last slider change before the shadow layer is re-solved.
const RESOLVE_DELAY: f64 = 0.35;

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

/// Layers of the layer panel (also the order of `App::visible` / `App::opacity`), top to bottom.
const SHADOW: usize = 0;
const AI: usize = 1;
const ORIGINAL: usize = 2;

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Running { started: f64 },
    Done { at: f64 },
    Stopped,
    Failed,
}

#[derive(Clone)]
struct Input {
    img: Arc<Image8>,
    tex: TextureHandle,
    name: String,
}

struct Entry {
    id: u64,
    /// Bumped whenever the layers are re-solved.
    serial: u64,
    input: Input,
    result: RunResult,
    thumb: TextureHandle,
}

/// Textures of the shown result.
struct Views {
    key: (u64, u64),
    stack_key: Option<StackKey>,
    /// The visible layers flattened: what the canvas shows.
    stack: TextureHandle,
    /// AI output at the original's size (RGBA), to blend with the other layers.
    ai_full: Vec<u8>,
    /// Layer panel thumbnails (shadow, AI output, original).
    thumbs: [TextureHandle; 3],
}

#[derive(PartialEq, Clone, Copy)]
struct StackKey {
    visible: [bool; 3],
    blend: usize,
    opacity: [u32; 3],
}

struct Resolve {
    id: u64,
    params: Params,
    rx: Receiver<Layers>,
}

struct Toast {
    ok: bool,
    text: String,
    at: f64,
    folder: Option<PathBuf>,
}

/// Self-screenshot for checking the layout: KAGE_SHOT=<out.png> KAGE_SCENE=<scene> [KAGE_IMAGE=<path>]
/// [KAGE_LANG=en] [KAGE_AUTO=generate|extract KAGE_SHADED=<path> KAGE_VIEW=multiply|normal|ai
/// KAGE_RESOLVE=<strength> KAGE_EXPORT=1].
struct Shot {
    out: PathBuf,
    frame: u32,
    /// Waiting for a run to finish before taking the shot.
    auto: Option<String>,
    started: bool,
    done_frame: Option<u32>,
    /// Show only this layer (and this blend mode) before the shot.
    solo: Option<(usize, Option<usize>)>,
    /// After the run: change the strength (re-solve), then export.
    resolve: Option<f32>,
    export: bool,
}

struct App {
    lang: Lang,
    screen: Screen,
    settings_tab: usize,
    data: PathBuf,
    cfg: Config,
    saved_cfg: Config,
    cfg_changed_at: f64,
    applied_dirs: (String, String),
    log: Log,
    verifier: Verifier,
    comfy: Comfy,

    image: Option<Input>,
    load_error: Option<String>,
    seed: String,

    phase: Phase,
    stage: usize,
    stage_changed_at: f64,
    waiting_comfy: bool,
    steps: (u32, u32),
    first_step_at: Option<f64>,
    last_step_at: f64,
    run_rx: Option<Receiver<Event>>,
    run_input: Option<Input>,
    cancel: Arc<AtomicBool>,
    error: Option<String>,

    history: Vec<Entry>,
    next_id: u64,
    current: Option<u64>,
    views: Option<Views>,
    resolve: Option<Resolve>,
    post_seen: (f32, bool, bool),
    post_changed_at: f64,

    visible: [bool; 3],
    selected_layer: usize,
    /// Per layer; the original's stays 1.
    opacity: [f32; 3],
    compare_on: bool,
    compare: f32,
    zoom: Option<f32>,
    pan: Vec2,
    /// Right panel: 0 layers, 1 history.
    right_tab: usize,
    export_open: bool,
    export_rx: Option<Receiver<Result<(usize, PathBuf), String>>>,
    toast: Option<Toast>,
    log_level: usize,
    log_seen: u64,
    adv_default_open: bool,
    shot: Option<Shot>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let data = config::data_dir();
        let (cfg, cfg_error) = Config::load(&data);
        let log = Log::new(&data.join("logs"));
        log.info(format!("Atozuke Kage MOD {} started (data: {})", env!("CARGO_PKG_VERSION"), data.display()));
        if let Some(e) = cfg_error {
            log.warn(e);
        }
        let verifier = Verifier::new(&data, log.clone());
        let comfy = Comfy::new(&data, log.clone());
        verifier.start(PathBuf::from(&cfg.model_dir));
        comfy.start(&cfg.comfy_dir, Path::new(&cfg.model_dir));
        let lang = if cfg.lang == "en" { Lang::En } else { Lang::Ja };
        Self {
            lang,
            screen: Screen::Main,
            settings_tab: 0,
            applied_dirs: (cfg.model_dir.clone(), cfg.comfy_dir.clone()),
            post_seen: (cfg.strength, cfg.gray, cfg.ghost),
            saved_cfg: cfg.clone(),
            cfg,
            cfg_changed_at: 0.0,
            data,
            log,
            verifier,
            comfy,
            image: None,
            load_error: None,
            seed: "random".into(),
            phase: Phase::Idle,
            stage: 0,
            stage_changed_at: 0.0,
            waiting_comfy: false,
            steps: (0, 0),
            first_step_at: None,
            last_step_at: 0.0,
            run_rx: None,
            run_input: None,
            cancel: Arc::new(AtomicBool::new(false)),
            error: None,
            history: Vec::new(),
            next_id: 1,
            current: None,
            views: None,
            resolve: None,
            post_changed_at: 0.0,
            visible: [true, false, true],
            selected_layer: SHADOW,
            opacity: [1.0; 3],
            compare_on: false,
            compare: 0.5,
            zoom: None,
            pan: Vec2::ZERO,
            right_tab: 0,
            export_open: false,
            export_rx: None,
            toast: None,
            log_level: 0,
            log_seen: 0,
            adv_default_open: false,
            shot: None,
        }
        .with_scene(&cc.egui_ctx)
    }

    fn with_scene(mut self, ctx: &egui::Context) -> Self {
        let Ok(out) = std::env::var("KAGE_SHOT") else { return self };
        let solo = match std::env::var("KAGE_VIEW").as_deref() {
            Ok("multiply") => Some((SHADOW, Some(0))),
            Ok("normal") => Some((SHADOW, Some(7))),
            Ok("ai") => Some((AI, None)),
            Ok("compare") => {
                self.compare_on = true;
                None
            }
            _ => None,
        };
        self.shot = Some(Shot {
            out: out.into(),
            frame: 0,
            auto: std::env::var("KAGE_AUTO").ok(),
            started: false,
            done_frame: None,
            solo,
            resolve: std::env::var("KAGE_RESOLVE").ok().and_then(|v| v.parse().ok()),
            export: std::env::var("KAGE_EXPORT").is_ok(),
        });
        if std::env::var("KAGE_LANG").as_deref() == Ok("en") {
            self.lang = Lang::En;
        }
        let scene = std::env::var("KAGE_SCENE").unwrap_or_default();
        if scene != "empty" {
            if let Ok(path) = std::env::var("KAGE_IMAGE") {
                self.open_path(ctx, Path::new(&path));
            }
        }
        match scene.as_str() {
            "advanced" => self.adv_default_open = true,
            "history" => self.right_tab = 1,
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
        ctx.request_repaint();
        let mut start: Option<String> = None;
        let mut export = false;
        let take = match &shot.auto {
            None => shot.frame == 15,
            Some(auto) => {
                if !shot.started && self.image.is_some() && (auto == "extract" || (self.verifier.all_ok() && self.comfy.url().is_some())) {
                    shot.started = true;
                    start = Some(auto.clone());
                }
                let shown_strength = self.current.and_then(|id| self.history.iter().find(|e| e.id == id)).map(|e| e.result.params.strength);
                let settled = matches!(self.phase, Phase::Done { .. } | Phase::Failed)
                    && self.resolve.is_none()
                    && self.export_rx.is_none()
                    && (shown_strength.is_none() || shown_strength == Some(self.cfg.strength));
                if shot.done_frame.is_none() && settled {
                    if let Some(k) = shot.resolve.take() {
                        self.cfg.strength = k;
                    } else if shot.export {
                        shot.export = false;
                        export = true;
                    } else {
                        shot.done_frame = Some(shot.frame);
                        if let Some((layer, blend)) = shot.solo {
                            self.visible = std::array::from_fn(|i| i == layer);
                            self.selected_layer = layer;
                            if let Some(b) = blend {
                                self.cfg.blend = b;
                            }
                        }
                    }
                }
                shot.done_frame.is_some_and(|f| shot.frame == f + 20)
            }
        };
        if take {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
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
        if export {
            self.export(ctx);
        }
        match start.as_deref() {
            Some("extract") => {
                if let Ok(p) = std::env::var("KAGE_SHADED") {
                    self.cfg.bg_shadow = std::env::var("KAGE_BG").as_deref() != Ok("0");
                    if let Ok(img) = load_image(Path::new(&p)) {
                        self.start_run(ctx, Some(Arc::new(img)));
                    }
                }
            }
            Some(_) => self.start_run(ctx, None),
            None => {}
        }
    }

    fn t(&self) -> &'static T {
        t(self.lang)
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        match load_image(path) {
            Ok(img) => {
                let tex = ctx.load_texture("input", color_image(&img.rgba, img.w, img.h, max_side(ctx)), egui::TextureOptions::LINEAR);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.log.info(format!("Opened {name} {}x{}", img.w, img.h));
                self.image = Some(Input { img: Arc::new(img), tex, name });
                self.load_error = None;
                self.zoom = None;
                self.pan = Vec2::ZERO;
                self.current = None;
                if !self.running() {
                    self.phase = Phase::Idle;
                }
            }
            Err(e) => {
                self.log.error(format!("Could not open {}: {e}", path.display()));
                self.load_error = Some(e);
            }
        }
    }

    fn pick_image(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]).pick_file() {
            self.open_path(ctx, &path);
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
    fn ready(&self) -> bool {
        self.verifier.all_ok() && matches!(self.comfy.state(), comfy::State::Ready { .. } | comfy::State::Starting)
    }

    fn start_run(&mut self, ctx: &egui::Context, shaded: Option<Arc<Image8>>) {
        let Some(input) = self.image.clone() else { return };
        // Keep random seeds below 2^53 so they survive any JSON round trip and are easy to copy.
        let seed = self.seed.trim().parse::<u64>().unwrap_or_else(|_| comfy::random_u64() >> 11);
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        let repaint = ctx.clone();
        pipeline::spawn(
            Request {
                original: input.img.clone(),
                name: input.name.clone(),
                params: self.params(seed),
                shaded,
                model_dir: PathBuf::from(&self.cfg.model_dir),
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
        let now = ctx.input(|i| i.time);
        self.run_rx = Some(rx);
        self.run_input = Some(input);
        self.phase = Phase::Running { started: now };
        self.stage = 0;
        self.stage_changed_at = now;
        self.waiting_comfy = false;
        self.steps = (0, 0);
        self.first_step_at = None;
        self.error = None;
    }

    fn entry(&self, id: Option<u64>) -> Option<&Entry> {
        id.and_then(|id| self.history.iter().find(|e| e.id == id))
    }

    fn select_entry(&mut self, id: u64) {
        let Some(e) = self.history.iter().find(|e| e.id == id) else { return };
        let p = &e.result.params;
        self.cfg.light_dir = p.light_dir;
        self.cfg.extra = p.extra.clone();
        self.cfg.strength = p.strength;
        self.cfg.bg_shadow = p.background;
        self.cfg.gray = p.gray;
        self.cfg.ghost = p.ghost;
        self.post_seen = (p.strength, p.gray, p.ghost);
        self.image = Some(e.input.clone());
        self.current = Some(id);
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let events: Vec<Event> = self.run_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for ev in events {
            match ev {
                Event::WaitingForComfy => self.waiting_comfy = true,
                Event::Stage(s) => {
                    self.waiting_comfy = false;
                    if s != self.stage {
                        self.stage = s;
                        self.stage_changed_at = now;
                    }
                }
                Event::Steps(v, m) => {
                    self.waiting_comfy = false;
                    if self.first_step_at.is_none() {
                        self.first_step_at = Some(now);
                    }
                    self.steps = (v, m);
                    self.last_step_at = now;
                }
                Event::Done(r) => {
                    let input = self.run_input.take().unwrap_or_else(|| self.image.clone().expect("input"));
                    let id = self.next_id;
                    self.next_id += 1;
                    let thumb = thumbnail(ctx, &r.layers.preview, r.layers.w, r.layers.h);
                    self.history.insert(0, Entry { id, serial: 0, input, result: *r, thumb });
                    self.history.truncate(30);
                    if self.image.as_ref().is_some_and(|i| Arc::ptr_eq(&i.img, &self.history[0].input.img)) {
                        self.current = Some(id);
                    }
                    self.phase = Phase::Done { at: now };
                    self.run_rx = None;
                    self.visible = [true, false, true];
                    self.selected_layer = SHADOW;
                }
                Event::Failed(msg) => {
                    self.error = Some(msg);
                    self.phase = Phase::Failed;
                    self.run_rx = None;
                }
                Event::Cancelled => {
                    self.phase = Phase::Stopped;
                    self.run_rx = None;
                }
            }
        }
        if self.running() {
            ctx.request_repaint();
        }
        if let Phase::Done { at } = self.phase {
            if now - at < 0.5 {
                ctx.request_repaint();
            }
        }
        self.tick_resolve(ctx, now);
        self.tick_export(ctx, now);
        self.tick_config(now);
    }

    /// Strength / gray / double-line changes re-solve the shown layer from the saved AI output (no new AI run).
    fn tick_resolve(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(r) = &self.resolve {
            if let Ok(layers) = r.rx.try_recv() {
                let (id, params) = (r.id, r.params.clone());
                self.resolve = None;
                if let Some(e) = self.history.iter_mut().find(|e| e.id == id) {
                    e.thumb = thumbnail(ctx, &layers.preview, layers.w, layers.h);
                    e.result.layers = layers;
                    e.result.params = params;
                    e.serial += 1;
                }
            }
            ctx.request_repaint();
            return;
        }
        let seen = (self.cfg.strength, self.cfg.gray, self.cfg.ghost);
        if seen != self.post_seen {
            self.post_seen = seen;
            self.post_changed_at = now;
        }
        if self.running() {
            return;
        }
        let Some(e) = self.entry(self.current) else { return };
        let p = &e.result.params;
        if (p.strength, p.gray, p.ghost) == seen {
            return;
        }
        if now - self.post_changed_at < RESOLVE_DELAY {
            ctx.request_repaint();
            return;
        }
        let params = Params { strength: seen.0, gray: seen.1, ghost: seen.2, ..p.clone() };
        let (original, shaded, mask, log) = (e.result.original.clone(), e.result.shaded.clone(), e.result.mask.clone(), self.log.clone());
        let (tx, rx) = mpsc::channel();
        let p2 = params.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let layers = pipeline::solve(&original, &shaded, mask.as_deref(), &p2, &log);
            let _ = tx.send(layers);
            repaint.request_repaint();
        });
        self.resolve = Some(Resolve { id: e.id, params, rx });
    }

    fn tick_config(&mut self, now: f64) {
        self.cfg.lang = if self.lang == Lang::En { "en" } else { "ja" }.into();
        if self.cfg != self.saved_cfg {
            if self.cfg_changed_at == 0.0 {
                self.cfg_changed_at = now;
            } else if now - self.cfg_changed_at > 1.0 {
                self.save_config();
            }
        }
    }

    fn save_config(&mut self) {
        if let Err(e) = self.cfg.save(&self.data) {
            self.log.error(format!("Could not save the settings: {e}"));
        }
        self.saved_cfg = self.cfg.clone();
        self.cfg_changed_at = 0.0;
    }

    /// Apply changed model / ComfyUI folders: re-verify and restart ComfyUI (it reads the model paths at start).
    fn apply_dirs(&mut self) {
        let dirs = (self.cfg.model_dir.trim().to_string(), self.cfg.comfy_dir.trim().to_string());
        if dirs == self.applied_dirs {
            return;
        }
        if dirs.0 != self.applied_dirs.0 {
            self.verifier.start(PathBuf::from(&dirs.0));
        }
        self.comfy.start(&dirs.1, Path::new(&dirs.0));
        self.applied_dirs = dirs;
    }

    fn stack_key(&self) -> StackKey {
        StackKey { visible: self.visible, blend: self.cfg.blend, opacity: self.opacity.map(|o| (o * 1000.0).round() as u32) }
    }

    fn ensure_views(&mut self, ctx: &egui::Context) {
        let stack_key = self.stack_key();
        // Field-level borrow so self.views can be replaced below.
        let Some(e) = self.current.and_then(|id| self.history.iter().find(|e| e.id == id)) else {
            self.views = None;
            return;
        };
        let key = (e.id, e.serial);
        let max = max_side(ctx);
        let (r, l) = (&e.result, &e.result.layers);
        if self.views.as_ref().map(|v| v.key) != Some(key) {
            let s = &r.shaded;
            let ai_full = export::to_canvas_rgba(&Pixels { w: s.w, h: s.h, channels: 4, data: &s.rgba }, l.w, l.h);
            self.views = Some(Views {
                key,
                stack_key: None,
                stack: ctx.load_texture("stack", egui::ColorImage::filled([1, 1], Color32::TRANSPARENT), egui::TextureOptions::LINEAR),
                ai_full,
                thumbs: [
                    thumbnail(ctx, &layer_rgba(&l.multiply, l.color), l.w, l.h),
                    thumbnail(ctx, &s.rgba, s.w, s.h),
                    thumbnail(ctx, &r.original.rgba, l.w, l.h),
                ],
            });
        }
        let views = self.views.as_mut().expect("views");
        if views.stack_key != Some(stack_key) {
            let rgba = flatten(r, &views.ai_full, self.visible, self.cfg.blend, self.opacity);
            views.stack.set(color_image(&rgba, l.w, l.h, max), egui::TextureOptions::LINEAR);
            views.stack_key = Some(stack_key);
        }
    }

    fn export(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let now = ctx.input(|i| i.time);
        let Some(e) = self.entry(self.current) else { return };
        let targets = self.cfg.export_targets;
        if !targets.iter().any(|&x| x) {
            self.toast = Some(Toast { ok: false, text: t.nothing_to_export.into(), at: now, folder: None });
            return;
        }
        let r = &e.result;
        let original = r.original.clone();
        let shaded = r.shaded.clone();
        let (w, h, color) = (r.layers.w, r.layers.h, r.layers.color);
        let multiply = r.layers.multiply.clone();
        let overlay = r.layers.overlay.clone();
        let (visible, opacity) = (self.visible, self.opacity);
        let mode = MODES[self.cfg.blend];
        let format = FORMATS[self.cfg.export_format.min(3)];
        let (png_level, quality) = (self.cfg.png_level, self.cfg.quality);
        let dir = PathBuf::from(self.cfg.out_dir.trim());
        let stem = Path::new(&r.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
        let names = t.layer_names;
        let log = self.log.clone();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(usize, PathBuf), String> {
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let lch = if color { 3 } else { 1 };
                let ai_full = export::to_canvas_rgba(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, w, h);
                // The composite is what the layer panel shows (visibility, blend mode, opacity).
                let composite = blend::flatten(
                    &blend::Stack {
                        original: visible[ORIGINAL].then_some(original.rgba.as_slice()),
                        ai: visible[AI].then_some((ai_full.as_slice(), opacity[AI])),
                        shadow: visible[SHADOW].then_some((multiply.as_slice(), lch, mode, opacity[SHADOW])),
                    },
                    w * h,
                );
                let mut written = Vec::new();
                if format == Format::Psd {
                    // Same layers as the panel; the normal-blend shadow is the black layer with alpha.
                    let shadow = if mode == blend::Mode::Normal { overlay.clone() } else { layer_rgba(&multiply, color) };
                    let layers = [
                        PsdLayer { name: names[ORIGINAL], rgba: original.rgba.clone(), mode: None, opacity: 1.0, visible: visible[ORIGINAL] },
                        PsdLayer { name: names[AI], rgba: ai_full, mode: None, opacity: opacity[AI], visible: visible[AI] },
                        PsdLayer { name: names[SHADOW], rgba: shadow, mode: Some(mode), opacity: opacity[SHADOW], visible: visible[SHADOW] },
                    ];
                    let path = export::unique_path(&dir, &stem, "kage", "psd");
                    export::save_psd(&path, w, h, &layers, &composite)?;
                    written.push(path);
                } else {
                    let ext = format.ext();
                    if targets[0] {
                        let path = export::unique_path(&dir, &stem, "shadow", ext);
                        export::save(&Pixels { w, h, channels: lch, data: &multiply }, &path, format, png_level, quality)?;
                        written.push(path);
                    }
                    if targets[1] {
                        let path = export::unique_path(&dir, &stem, "ai", ext);
                        export::save(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, &path, format, png_level, quality)?;
                        written.push(path);
                    }
                    if targets[2] {
                        let path = export::unique_path(&dir, &stem, "shaded", ext);
                        export::save(&Pixels { w, h, channels: 4, data: &composite }, &path, format, png_level, quality)?;
                        written.push(path);
                    }
                }
                for p in &written {
                    log.info(format!("Exported {}", p.display()));
                }
                Ok((written.len(), dir.clone()))
            })();
            if let Err(e) = &result {
                log.error(format!("Export failed: {e}"));
            }
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.export_rx = Some(rx);
    }

    fn tick_export(&mut self, ctx: &egui::Context, now: f64) {
        let t = self.t();
        if let Some(rx) = &self.export_rx {
            if let Ok(result) = rx.try_recv() {
                self.export_rx = None;
                self.toast = Some(match result {
                    Ok((n, dir)) => Toast { ok: true, text: t.exported.replace("{}", &n.to_string()), at: now, folder: Some(dir) },
                    Err(e) => Toast { ok: false, text: format!("{}: {e}", t.export_failed), at: now, folder: None },
                });
            }
        }
        if let Some(toast) = &self.toast {
            if now - toast.at > 6.0 {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }

    // ---- main screen -------------------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let has_result = self.current.is_some();
        ui.horizontal_centered(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  {}", ph::GEAR_SIX, t.settings)).on_hover_text(t.settings_tip).clicked() {
                    self.screen = Screen::Settings;
                }
                let (text, color, tip, tab) = self.readiness(t);
                let r = ui.add(egui::Label::new(RichText::new(format!("●  {text}")).color(color)).sense(Sense::click()));
                if r.on_hover_text(tip).clicked() {
                    if let Some(tab) = tab {
                        self.screen = Screen::Settings;
                        self.settings_tab = tab;
                    }
                }
                self.language_combo(ui);
                ui.separator();
                if ui
                    .add_enabled(has_result && self.export_rx.is_none(), Button::icon_text(ph::DOWNLOAD_SIMPLE, t.export))
                    .on_hover_text(t.export_tip)
                    .clicked()
                {
                    self.export_open = true;
                }
            });
        });
    }

    /// Toolbar status: text, color, tooltip and the settings tab it leads to.
    fn readiness(&self, t: &'static T) -> (&'static str, Color32, &'static str, Option<usize>) {
        if self.verifier.running() {
            return (t.model_checking, BLUE, t.model_missing_tip, Some(0));
        }
        if !self.verifier.all_ok() {
            return (t.model_missing, AMBER, t.model_missing_tip, Some(0));
        }
        match self.comfy.state() {
            comfy::State::Ready { .. } => (t.model_ok, GREEN, t.model_ok_tip, None),
            comfy::State::Starting => (t.comfy_starting, BLUE, t.comfy_not_ready_tip, Some(1)),
            _ => (t.comfy_not_ready, AMBER, t.comfy_not_ready_tip, Some(1)),
        }
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
        let running = self.running();
        let ready = self.ready();

        egui::Panel::bottom("generate").frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(0, 10))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 84.0 - ui.spacing().item_spacing.x;
                let gen = egui::Button::new(RichText::new(format!("{}  {}", ph::PLAY, t.generate)).size(16.0).strong().color(BLUE))
                    .fill(SEL)
                    .stroke(Stroke::new(1.0, BLUE));
                let enabled = self.image.is_some() && !running && ready;
                let resp = ui.add_enabled_ui(enabled, |ui| ui.add_sized([w, 48.0], gen)).inner;
                let resp = if !ready { resp.on_disabled_hover_text(t.not_ready_tip) } else { resp.on_hover_text(t.generate_tip) };
                if resp.clicked() {
                    self.start_run(&ctx, None);
                }
                let stop = egui::Button::new(RichText::new(format!("{}  {}", ph::STOP, t.stop)).size(14.0));
                if ui.add_enabled_ui(running, |ui| ui.add_sized([84.0, 48.0], stop)).inner.on_hover_text(t.stop_tip).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(10.0);
            // Open / drop box
            let inner = Frame::new().fill(Color32::from_rgb(0xfa, 0xfa, 0xfa)).inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let open = egui::Button::new(RichText::new(format!("{}  {}", ph::FOLDER_OPEN, t.open_image)).size(15.0).strong());
                if ui.add_sized([ui.available_width(), 44.0], open).on_hover_text(t.open_tip).clicked() {
                    self.pick_image(&ctx);
                }
                ui.vertical_centered(|ui| ui.label(RichText::new(t.drop_hint).small().color(MUTED)));
                if let Some(img) = &self.image {
                    ui.separator();
                    let row = Vec2::new(ui.available_width(), 20.0);
                    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}×{}", img.img.w, img.img.h)).monospace().color(MUTED));
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(&img.name).color(TEXT)).truncate()).on_hover_text(&img.name);
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
                    let b = egui::Button::new(RichText::new(*icon).size(17.0)).selected(self.cfg.light_dir == i);
                    if ui.add_sized([w, 34.0], b).on_hover_text(t.light_tips[i]).clicked() {
                        self.cfg.light_dir = i;
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
                    ui.label(RichText::new(format!("× {:.1}", self.cfg.strength)).monospace());
                });
            });
            ui.spacing_mut().slider_width = ui.available_width();
            ui.add(egui::Slider::new(&mut self.cfg.strength, 0.5..=1.5).step_by(0.1).show_value(false));

            ui.add_space(10.0);
            ui.checkbox(&mut self.cfg.bg_shadow, t.bg_shadow).on_hover_text(t.bg_tip);
            ui.checkbox(&mut self.cfg.gray, t.gray).on_hover_text(t.gray_tip);

            ui.add_space(8.0);
            ui.separator();
            egui::CollapsingHeader::new(RichText::new(t.advanced).strong())
                .id_salt("advanced")
                .default_open(self.adv_default_open)
                .show(ui, |ui| {
                    ui.label(t.extra);
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.extra).hint_text("warm sunset light").desired_width(f32::INFINITY));
                    ui.label(RichText::new(t.extra_desc).small().color(MUTED));
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.cfg.ghost, t.ghost);
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
                    let can_extract = self.image.is_some() && !running && (self.cfg.bg_shadow || ready);
                    if ui.add_enabled(can_extract, b.min_size(Vec2::new(ui.available_width(), 28.0))).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title(t.pick_shaded)
                            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                            .pick_file()
                        {
                            match load_image(&path) {
                                Ok(img) => self.start_run(&ctx, Some(Arc::new(img))),
                                Err(e) => self.load_error = Some(e),
                            }
                        }
                    }
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

        let Some(img) = self.image.clone() else {
            let inner = rect.shrink(24.0);
            dashed_rect(&painter, inner, Color32::from_gray(0x9a));
            painter.text(inner.center() - Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, ph::UPLOAD_SIMPLE, FontId::proportional(40.0), MUTED);
            painter.text(inner.center() + Vec2::new(0.0, 24.0), Align2::CENTER_CENTER, t.drop_here, FontId::proportional(15.0), MUTED);
            return;
        };
        let has_result = self.views.is_some();

        // Pan (drag) and zoom (wheel, around the cursor).
        let size = Vec2::new(img.img.w as f32, img.img.h as f32);
        let fit = ((rect.width() - 48.0) / size.x).min((rect.height() - 48.0) / size.y);
        let mut scale = self.zoom.unwrap_or(fit);
        let mut divider_drag = false;
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        let divider_x = img_rect.left() + img_rect.width() * self.compare;
        let compare_active = has_result && self.compare_on;
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
        match &self.views {
            Some(v) => {
                // Hidden layers can leave the stack transparent.
                checkerboard(&painter, img_rect.intersect(rect));
                let split = if compare_active { self.compare } else { 1.0 };
                let left = Rect::from_min_max(img_rect.min, Pos2::new(img_rect.left() + img_rect.width() * split, img_rect.bottom()));
                painter.image(v.stack.id(), left, Rect::from_min_max(Pos2::ZERO, Pos2::new(split, 1.0)), Color32::WHITE);
                if compare_active {
                    // Right of the divider: the original alone.
                    let right = Rect::from_min_max(Pos2::new(left.right(), img_rect.top()), img_rect.max);
                    painter.image(img.tex.id(), right, Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0)), Color32::WHITE);
                    let x = left.right();
                    painter.line_segment([Pos2::new(x, img_rect.top()), Pos2::new(x, img_rect.bottom())], Stroke::new(2.0, Color32::WHITE));
                    let knob = Rect::from_center_size(Pos2::new(x, img_rect.center().y), Vec2::new(22.0, 34.0));
                    painter.rect_filled(knob, CornerRadius::same(2), Color32::WHITE);
                    painter.rect_stroke(knob, CornerRadius::same(2), Stroke::new(1.0, Color32::from_gray(0x8a)), StrokeKind::Inside);
                    painter.text(knob.center(), Align2::CENTER_CENTER, ph::ARROWS_LEFT_RIGHT, FontId::proportional(14.0), TEXT);
                }
            }
            None => {
                if img.img.has_alpha {
                    checkerboard(&painter, img_rect.intersect(rect));
                }
                painter.image(img.tex.id(), img_rect, full_uv, Color32::WHITE);
            }
        }

        self.canvas_overlays(&ctx, rect, scale, has_result);
    }

    fn canvas_overlays(&mut self, ctx: &egui::Context, rect: Rect, scale: f32, has_result: bool) {
        let t = self.t();
        let overlay = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, LINE_DARK))
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::symmetric(6, 4));

        // View tools (bottom right): compare, zoom.
        egui::Area::new(Id::new("zoom"))
            .fixed_pos(rect.right_bottom() + Vec2::new(-8.0, -8.0))
            .pivot(Align2::RIGHT_BOTTOM)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let compare = egui::Button::new(format!("{}  {}", ph::SQUARE_SPLIT_HORIZONTAL, t.compare)).selected(has_result && self.compare_on);
                        if ui.add_enabled(has_result, compare).on_hover_text(t.compare_tip).clicked() {
                            self.compare_on = !self.compare_on;
                        }
                        ui.separator();
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
                    });
                });
            });

        // Export result (top center)
        if let Some(toast) = &self.toast {
            let mut open_dir = None;
            egui::Area::new(Id::new("toast"))
                .fixed_pos(rect.center_top() + Vec2::new(0.0, 12.0))
                .pivot(Align2::CENTER_TOP)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    overlay.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = if toast.ok { (ph::CHECK_CIRCLE, GREEN) } else { (ph::WARNING_CIRCLE, RED) };
                            ui.label(RichText::new(icon).size(16.0).color(color));
                            ui.label(&toast.text);
                            if let Some(dir) = &toast.folder {
                                if ui.button(format!("{}  {}", ph::FOLDER_OPEN, t.open_folder)).clicked() {
                                    open_dir = Some(dir.clone());
                                }
                            }
                        });
                    });
                });
            if let Some(dir) = open_dir {
                open_in_explorer(&dir);
            }
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            for (i, label) in [t.tab_layers, t.history].into_iter().enumerate() {
                if ui.add(egui::Button::new(label).selected(self.right_tab == i).min_size(Vec2::new(96.0, 26.0))).clicked() {
                    self.right_tab = i;
                }
            }
        });
        ui.separator();
        match self.right_tab {
            0 => self.layers(ui),
            _ => self.history(ui),
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

        egui::Grid::new("layer_props").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(t.blend_title);
            ui.add_enabled_ui(has_result && sel == SHADOW, |ui| {
                let text = if sel == SHADOW { t.blends[self.cfg.blend] } else { t.normal };
                egui::ComboBox::from_id_salt("blend").width(176.0).selected_text(text).show_ui(ui, |ui| {
                    for i in MENU_ORDER {
                        ui.selectable_value(&mut self.cfg.blend, i, t.blends[i]);
                    }
                });
            });
            ui.end_row();
            ui.label(t.opacity);
            ui.add_enabled_ui(has_result && sel != ORIGINAL, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 128.0;
                    ui.add(egui::Slider::new(&mut self.opacity[sel], 0.0..=1.0).show_value(false));
                    ui.label(RichText::new(format!("{:>3.0}%", self.opacity[sel] * 100.0)).monospace());
                });
            });
            ui.end_row();
        });
        ui.separator();

        if ids.is_empty() {
            ui.add(egui::Label::new(RichText::new(t.no_layers).color(MUTED)).wrap());
            return;
        }
        let mut select = None;
        let mut eye: Option<(usize, bool)> = None;
        for &id in &ids {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), Sense::click());
            if sel == id {
                ui.painter().rect_filled(r, CornerRadius::same(2), SEL);
                ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, BLUE), StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(2), Color32::from_gray(0xe6));
            }
            let visible = self.visible[id];
            // Eye (registered after the row, so it gets its own clicks).
            let eye_rect = Rect::from_min_size(Pos2::new(r.left() + 4.0, r.center().y - 14.0), Vec2::splat(28.0));
            let eye_resp = ui.interact(eye_rect, Id::new(("eye", id)), Sense::click());
            let eye_fill = if eye_resp.hovered() { Color32::from_gray(0xe6) } else { Color32::from_gray(0xf8) };
            ui.painter().rect_filled(eye_rect, CornerRadius::same(2), eye_fill);
            ui.painter().rect_stroke(eye_rect, CornerRadius::same(2), Stroke::new(1.0, LINE), StrokeKind::Inside);
            let (icon, icon_color) = if visible { (ph::EYE, TEXT) } else { (ph::EYE_SLASH, Color32::from_gray(0x9a)) };
            ui.painter().text(eye_rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(16.0), icon_color);
            let label = format!("{}: {}", if visible { t.hide } else { t.show }, t.layer_names[id]);
            if eye_resp.on_hover_text(format!("{label}\n{}", t.eye_tip)).clicked() {
                eye = Some((id, ui.input(|i| i.modifiers.alt)));
            } else if resp.clicked() {
                select = Some(id);
            }
            // Thumbnail.
            let slot = Rect::from_min_size(Pos2::new(eye_rect.right() + 8.0, r.top() + 5.0), Vec2::new(34.0, 48.0));
            if let Some(v) = &self.views {
                let tex = &v.thumbs[id];
                let ts = tex.size_vec2();
                let k = (slot.width() / ts.x).min(slot.height() / ts.y);
                let thumb = Rect::from_center_size(slot.center(), ts * k);
                let tint = if visible { Color32::WHITE } else { Color32::from_white_alpha(110) };
                ui.painter().image(tex.id(), thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
                ui.painter().rect_stroke(thumb, CornerRadius::ZERO, Stroke::new(1.0, LINE_DARK), StrokeKind::Outside);
            } else if let Some(img) = &self.image {
                let ts = Vec2::new(img.img.w as f32, img.img.h as f32);
                let k = (slot.width() / ts.x).min(slot.height() / ts.y);
                let thumb = Rect::from_center_size(slot.center(), ts * k);
                ui.painter().image(img.tex.id(), thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(thumb, CornerRadius::ZERO, Stroke::new(1.0, LINE_DARK), StrokeKind::Outside);
            }
            // Name and blend / opacity.
            let x = slot.right() + 10.0;
            let painter = ui.painter_at(r.shrink(2.0));
            painter.text(Pos2::new(x, r.top() + 20.0), Align2::LEFT_CENTER, t.layer_names[id], FontId::proportional(13.0), if visible { TEXT } else { MUTED });
            let sub = match id {
                ORIGINAL => t.background.to_string(),
                _ => {
                    let mode = if id == SHADOW { t.blends[self.cfg.blend] } else { t.normal };
                    format!("{mode} · {:.0}%", self.opacity[id] * 100.0)
                }
            };
            painter.text(Pos2::new(x, r.top() + 40.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.5), MUTED);
            if id == ORIGINAL {
                let lock = Pos2::new(r.right() - 16.0, r.center().y);
                painter.text(lock, Align2::CENTER_CENTER, ph::LOCK_SIMPLE, FontId::proportional(14.0), MUTED);
                ui.interact(Rect::from_center_size(lock, Vec2::splat(18.0)), Id::new("lock"), Sense::hover()).on_hover_text(t.locked);
            }
        }
        if let Some(id) = select {
            self.selected_layer = id;
        }
        if let Some((id, solo)) = eye {
            if solo {
                self.visible = std::array::from_fn(|i| i == id);
                self.selected_layer = id;
            } else {
                self.visible[id] = !self.visible[id];
            }
        }
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(t.solo_hint).small().color(MUTED)).wrap());
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add(egui::Label::new(RichText::new(t.psd_note).small().color(MUTED)).wrap());
            ui.separator();
        });
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        // "Auto (match the image's light)" -> "Auto": the drawer is narrow.
        let dir_short = |i: usize| {
            let s = t.light_tips[i].split(": ").last().unwrap_or("");
            s.split(['（', '(']).next().unwrap_or(s).trim_end()
        };
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in &self.history {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 74.0), Sense::click());
                if self.current == Some(e.id) {
                    ui.painter().rect_filled(r, CornerRadius::same(2), SEL);
                    ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, BLUE), StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(2), Color32::from_gray(0xe6));
                }
                let slot = Rect::from_min_size(r.min + Vec2::new(4.0, 4.0), Vec2::new(37.0, 66.0));
                let ts = e.thumb.size_vec2();
                let k = (slot.width() / ts.x).min(slot.height() / ts.y);
                let thumb = Rect::from_center_size(slot.center(), ts * k);
                ui.painter().image(e.thumb.id(), thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                let x = slot.right() + 8.0;
                let painter = ui.painter_at(r.shrink(2.0));
                let title = if self.lang == Lang::Ja { format!("{} {}", e.id, t.run_n) } else { format!("{} {}", t.run_n, e.id) };
                painter.text(Pos2::new(x, r.top() + 18.0), Align2::LEFT_CENTER, title, FontId::proportional(13.0), TEXT);
                let p = &e.result.params;
                let line2 = if e.result.extracted { t.extract.to_string() } else { dir_short(p.light_dir).to_string() };
                painter.text(Pos2::new(x, r.top() + 39.0), Align2::LEFT_CENTER, line2, FontId::proportional(12.0), MUTED);
                let sub = format!("×{:.1} · {}", p.strength, if p.background { t.with_bg } else { t.subject_only });
                painter.text(Pos2::new(x, r.top() + 56.0), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), MUTED);
                let tip = format!("{}  ·  {} {}  ·  {:.1} s", e.input.name, t.seed, p.seed, e.result.seconds);
                if resp.on_hover_text(tip).clicked() {
                    clicked = Some(e.id);
                }
            }
        });
        if let Some(id) = clicked {
            self.select_entry(id);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        let now = ui.input(|i| i.time);
        let running = self.running();
        let resolving = self.resolve.is_some();
        let p = match self.phase {
            Phase::Running { .. } => match self.stage {
                0 if self.steps.1 > 0 => AI_SHARE * self.steps.0 as f64 / self.steps.1 as f64,
                0 => 0.0,
                1 => 0.88,
                2 => 0.95,
                _ => 0.99,
            },
            Phase::Done { .. } => 1.0,
            _ => 0.0,
        };
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            match self.phase {
                _ if running || resolving => {
                    ui.add(egui::Spinner::new().size(16.0).color(BLUE));
                }
                Phase::Done { at } => {
                    let k = ((now - at) / 0.35).clamp(0.0, 1.0) as f32;
                    let pop = if k < 0.7 { 0.4 + k / 0.7 * 0.75 } else { 1.15 - (k - 0.7) / 0.3 * 0.15 };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, ph::CHECK_CIRCLE, FontId::proportional(18.0 * pop), GREEN);
                }
                Phase::Failed => {
                    ui.label(RichText::new(ph::WARNING_CIRCLE).size(16.0).color(RED));
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
                Phase::Running { .. } if self.waiting_comfy => t.waiting_comfy,
                Phase::Running { .. } => t.running[self.stage],
                _ if resolving => t.resolving,
                Phase::Done { .. } => t.done,
                Phase::Stopped => t.stopped,
                Phase::Failed => t.failed,
                Phase::Idle => t.idle,
            };
            let font = FontId::proportional(13.0);
            let color = if matches!(self.phase, Phase::Failed) && !running { RED } else { TEXT };
            let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), color);
            let dots_w = 16.0;
            let (r, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + dots_w, 20.0), Sense::hover());
            let k = if running { ((now - self.stage_changed_at) / 0.35).clamp(0.0, 1.0) as f32 } else { 1.0 };
            let ease = 1.0 - (1.0 - k).powi(3);
            let pos = Pos2::new(r.left(), r.center().y - galley.size().y / 2.0 + (1.0 - ease) * 6.0);
            ui.painter().galley(pos, galley.clone(), color.gamma_multiply(ease));
            if running || resolving {
                for d in 0..3 {
                    let phase = ((now * 1.6 - d as f64 * 0.2).rem_euclid(1.2) / 1.2) as f32;
                    let a = if phase < 0.4 { 0.15 + phase / 0.4 * 0.85 } else { 1.0 - (phase - 0.4) / 0.6 * 0.85 };
                    let x = r.left() + galley.size().x + 2.0 + d as f32 * 4.5;
                    ui.painter().circle_filled(Pos2::new(x, r.center().y + 4.0), 1.3, TEXT.gamma_multiply(a));
                }
            }

            if matches!(self.phase, Phase::Failed) {
                if let Some(err) = &self.error {
                    let w = (ui.available_width() - 120.0).max(80.0);
                    ui.add_sized([w, 20.0], egui::Label::new(RichText::new(err).color(RED)).truncate()).on_hover_text(err);
                }
            } else {
                let detail = match self.phase {
                    Phase::Running { .. } if self.stage == 0 && !self.waiting_comfy && self.steps.1 == 0 => t.loading_models.to_string(),
                    Phase::Running { .. } if self.stage == 0 && self.steps.1 > 0 => format!("{}/{} {}", self.steps.0, self.steps.1, t.steps),
                    Phase::Done { .. } => self.entry(self.current).map(|e| format!("{}×{}", e.result.layers.w, e.result.layers.h)).unwrap_or_default(),
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
                    let (top, bottom) = (fill.top(), fill.bottom());
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
                    Phase::Running { started } => {
                        let el = now - started;
                        let left = match (self.first_step_at, self.steps) {
                            (Some(first), (v, m)) if self.stage == 0 && v >= 2 => {
                                let per = (self.last_step_at - first) / (v - 1) as f64;
                                Some((m - v) as f64 * per + AFTER_STEPS_SECONDS - (now - self.last_step_at).min(per))
                            }
                            _ => None,
                        };
                        match left {
                            Some(left) => format!("{} {} · {} {}", clock(el), t.elapsed, t.left, clock(left)),
                            None => format!("{} {}", clock(el), t.elapsed),
                        }
                    }
                    Phase::Done { .. } => self.entry(self.current).map(|e| format!("{:.1} s", e.result.seconds)).unwrap_or_default(),
                    _ => String::new(),
                };
                ui.label(RichText::new(time).monospace().color(MUTED));
            }

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
        let mut go = false;
        let resp = egui::Modal::new(Id::new("export")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(t.export).strong().size(15.0));
            ui.separator();
            for (i, label) in t.export_targets.iter().enumerate() {
                ui.checkbox(&mut self.cfg.export_targets[i], *label);
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.format).strong());
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 9.0) / 4.0;
                for (i, name) in ["PNG", "JPEG", "WebP", "PSD"].iter().enumerate() {
                    if ui.add_sized([w, 28.0], egui::Button::new(*name).selected(self.cfg.export_format == i)).clicked() {
                        self.cfg.export_format = i;
                    }
                }
            });
            ui.add_space(10.0);
            ui.spacing_mut().slider_width = 360.0;
            match self.cfg.export_format {
                0 => {
                    ui.label(RichText::new(t.png_level).strong());
                    ui.add(egui::Slider::new(&mut self.cfg.png_level, 0..=9));
                }
                1 | 2 => {
                    ui.label(RichText::new(t.quality).strong());
                    ui.add(egui::Slider::new(&mut self.cfg.quality, 1..=100));
                }
                _ => {}
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t.out_dir).strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.cfg.out_dir).desired_width(ui.available_width() - 70.0).font(egui::TextStyle::Monospace));
                if ui.button(t.browse).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_directory(&self.cfg.out_dir).pick_folder() {
                        self.cfg.out_dir = dir.display().to_string();
                    }
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(t.do_export).strong().color(BLUE)).fill(SEL).stroke(Stroke::new(1.0, BLUE));
                if ui.add(b).clicked() {
                    go = true;
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
        if go {
            self.export(ctx);
        }
    }

    // ---- settings screen ---------------------------------------------------------------------------

    fn settings_toolbar(&mut self, ui: &mut egui::Ui) {
        let t = self.t();
        ui.horizontal_centered(|ui| {
            if ui.button(format!("{}  {}", ph::ARROW_LEFT, t.back)).on_hover_text(t.back_tip).clicked() {
                self.apply_dirs();
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
        let statuses = self.verifier.statuses();
        let notes = self.verifier.notes();
        let checking = self.verifier.running();
        let ok = statuses.iter().filter(|s| **s == Status::Ok).count();
        if checking {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.models).strong().size(15.0));
            ui.label(RichText::new(format!("{ok}/{} {}", MODELS.len(), t.match_word)).color(if ok == MODELS.len() { GREEN } else { AMBER }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(format!("{}  {}", ph::ARROWS_CLOCKWISE, t.recheck)).color(BLUE))
                    .fill(SEL)
                    .stroke(Stroke::new(1.0, BLUE));
                if ui.add_enabled(!checking, b).on_hover_text(t.recheck_tip).clicked() {
                    self.verifier.start(PathBuf::from(self.cfg.model_dir.trim()));
                    // A file that just became valid must be picked up by ComfyUI's model list.
                    if !matches!(self.comfy.state(), comfy::State::Ready { .. } | comfy::State::Starting) {
                        self.comfy.start(&self.cfg.comfy_dir, Path::new(self.cfg.model_dir.trim()));
                    }
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
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.cfg.model_dir).desired_width(ui.available_width() - 130.0).font(egui::TextStyle::Monospace),
            );
            if r.lost_focus() {
                self.apply_dirs();
            }
            if ui.button(t.browse).clicked() {
                if let Some(dir) = rfd::FileDialog::new().set_directory(&self.cfg.model_dir).pick_folder() {
                    self.cfg.model_dir = dir.display().to_string();
                    self.apply_dirs();
                }
            }
            if ui.button(t.open).on_hover_text(t.open_folder_tip).clicked() {
                let dir = PathBuf::from(self.cfg.model_dir.trim());
                for m in &MODELS {
                    let _ = std::fs::create_dir_all(dir.join(m.folder));
                }
                open_in_explorer(&dir);
            }
        });
        ui.label(RichText::new(t.folder_layout).small().color(MUTED));
        ui.add_space(8.0);
        Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("models").num_columns(6).spacing([14.0, 8.0]).show(ui, |ui| {
                for h in ["", t.col_file, t.col_dir, t.col_size, t.col_hash, t.col_source] {
                    ui.label(RichText::new(h).color(MUTED));
                }
                ui.end_row();
                for (i, m) in MODELS.iter().enumerate() {
                    let (icon, color, text) = match &statuses[i] {
                        Status::Ok => (ph::CHECK_CIRCLE, GREEN, t.states[0].to_string()),
                        Status::Checking(f) => (ph::CIRCLE_NOTCH, BLUE, t.states[1].replace("{}", &format!("{:.0}", f * 100.0))),
                        Status::Missing => (ph::CIRCLE_DASHED, AMBER, t.states[2].to_string()),
                        Status::Mismatch => (ph::X_CIRCLE, RED, t.states[3].to_string()),
                        Status::Unchecked => (ph::CIRCLE, MUTED, t.states[4].to_string()),
                        Status::Copying(f) => (ph::CIRCLE_NOTCH, BLUE, t.states[5].replace("{}", &format!("{:.0}", f * 100.0))),
                    };
                    ui.label(RichText::new(icon).size(17.0).color(color));
                    // Grid columns keep the width of their first layout, so cells must not wrap.
                    ui.vertical(|ui| {
                        ui.set_min_width(300.0);
                        ui.add(egui::Label::new(t.purposes[i]).extend());
                        ui.add(egui::Label::new(RichText::new(m.file).monospace().small().color(MUTED)).extend());
                    });
                    ui.add(egui::Label::new(RichText::new(m.folder).monospace().color(MUTED)).extend());
                    ui.add(egui::Label::new(RichText::new(format!("{:.2} GB", m.size as f64 / 1e9)).monospace()).extend());
                    let note = match &notes[i] {
                        Some(models::Note::PickedMismatch) => Some(t.picked_mismatch.to_string()),
                        Some(models::Note::PickedFailed(e)) => Some(t.picked_failed.replace("{}", e)),
                        None => None,
                    };
                    let state = egui::Label::new(RichText::new(text).color(color)).extend();
                    match note {
                        Some(note) => {
                            ui.vertical(|ui| {
                                ui.add(state).on_hover_text(m.sha256);
                                ui.add(egui::Label::new(RichText::new(note).small().color(RED)).extend());
                            });
                        }
                        None => {
                            ui.add(state).on_hover_text(m.sha256);
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  {}", ph::LINK, t.source_page)).on_hover_text(m.source).clicked() {
                            open_in_explorer(Path::new(m.source));
                        }
                        let pick = egui::Button::new(format!("{}  {}", ph::FILE_ARROW_UP, t.pick_file));
                        if ui.add_enabled(!checking, pick).on_hover_text(t.pick_file_tip).clicked() {
                            let mut dialog = rfd::FileDialog::new()
                                .set_title(format!("{} — {}", t.pick_file, m.file))
                                .add_filter("safetensors", &["safetensors"]);
                            if let Some(downloads) = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")) {
                                dialog = dialog.set_directory(downloads);
                            }
                            if let Some(src) = dialog.pick_file() {
                                self.verifier.place(i, src, PathBuf::from(self.cfg.model_dir.trim()));
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.label(RichText::new(t.only_safetensors).color(MUTED));
            ui.add_space(12.0);
            ui.label(RichText::new(t.total.replace("{}", &format!("{:.1}", models::total_bytes() as f64 / 1e9))).color(MUTED));
        });
    }

    fn comfy_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let state = self.comfy.state();
        if state == comfy::State::Starting {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        ui.label(RichText::new("ComfyUI").strong().size(15.0));
        ui.label(RichText::new(t.comfy_install).color(MUTED));
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
                if state == comfy::State::Starting {
                    ui.add(egui::Spinner::new().size(14.0).color(BLUE));
                } else {
                    ui.label(RichText::new("●").color(color));
                }
                ui.label(RichText::new(text).color(color));
                if !self.cfg.comfy_dir.trim().is_empty() && ui.button(format!("{}  {}", ph::ARROWS_CLOCKWISE, t.restart)).on_hover_text(t.restart_tip).clicked() {
                    self.comfy.start(&self.cfg.comfy_dir, Path::new(self.cfg.model_dir.trim()));
                    self.applied_dirs = (self.cfg.model_dir.trim().to_string(), self.cfg.comfy_dir.trim().to_string());
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
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(t.gpu_memory).strong());
        ui.checkbox(&mut self.cfg.keep_loaded, t.keep_loaded);
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.add(egui::Label::new(RichText::new(t.keep_loaded_desc).color(MUTED)).wrap());
        });
    }

    fn log_tab(&mut self, ui: &mut egui::Ui, t: &'static T) {
        let (serial, lines) = self.log.snapshot();
        let filter = [None, Some(Level::Info), Some(Level::Warn), Some(Level::Error)][self.log_level];
        let shown: Vec<_> = lines.iter().filter(|l| filter.is_none_or(|f| f == l.level)).collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.log).strong().size(15.0));
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
        Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::same(8)).show(ui, |ui| {
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped: Vec<_> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(path) = dropped.first() {
            self.open_path(&ctx, path);
        }
        self.tick(&ctx);
        self.handle_shot(&ctx);
        if self.verifier.running() || self.comfy.state() == comfy::State::Starting {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        let bar = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(6, 4)).stroke(Stroke::new(1.0, LINE));
        let side = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, LINE));
        match self.screen {
            Screen::Main => {
                egui::Panel::top("toolbar").frame(bar).exact_size(36.0).show(ui, |ui| self.toolbar(ui));
                egui::Panel::bottom("status").frame(bar).exact_size(40.0).show(ui, |ui| self.status_bar(ui));
                egui::Panel::left("controls").frame(side).exact_size(270.0).resizable(false).show(ui, |ui| self.controls(ui));
                self.ensure_views(&ctx);
                egui::Panel::right("right")
                    .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(8)).stroke(Stroke::new(1.0, LINE)))
                    .exact_size(280.0)
                    .resizable(false)
                    .show(ui, |ui| self.right_panel(ui));
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
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(12))).show(ui, |ui| self.settings_body(ui));
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.store(true, Ordering::SeqCst);
        self.cfg.lang = if self.lang == Lang::En { "en" } else { "ja" }.into();
        self.save_config();
        self.comfy.stop();
    }
}

/// Button with an icon glyph followed by a label.
struct Button;
impl Button {
    fn icon_text(icon: &str, text: &str) -> egui::Button<'static> {
        egui::Button::new(format!("{icon}  {text}"))
    }
}

/// Decode an image file, applying its EXIF orientation (as ComfyUI's LoadImage does).
fn load_image(path: &Path) -> Result<Image8, String> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    Ok(Image8::from_dynamic(&img))
}

fn max_side(ctx: &egui::Context) -> usize {
    ctx.input(|i| i.max_texture_side).max(2048)
}

/// RGBA texture data, downscaled when the GPU cannot hold the full size (display only).
fn color_image(rgba: &[u8], w: usize, h: usize, max: usize) -> egui::ColorImage {
    if w.max(h) <= max {
        return egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
    }
    let k = max as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1));
    egui::ColorImage::from_rgba_unmultiplied([nw, nh], &resample::resize_rgba(rgba, w, h, nw, nh, Filter::Bilinear))
}

/// The visible layers of a result, flattened (what the canvas shows and the composite export writes).
fn flatten(r: &RunResult, ai_full: &[u8], visible: [bool; 3], blend_idx: usize, opacity: [f32; 3]) -> Vec<u8> {
    let l = &r.layers;
    let stack = blend::Stack {
        original: visible[ORIGINAL].then_some(r.original.rgba.as_slice()),
        ai: visible[AI].then_some((ai_full, opacity[AI])),
        shadow: visible[SHADOW].then_some((l.multiply.as_slice(), if l.color { 3 } else { 1 }, MODES[blend_idx], opacity[SHADOW])),
    };
    blend::flatten(&stack, l.w * l.h)
}

/// Multiply layer (gray or RGB) as RGBA.
fn layer_rgba(layer: &[u8], color: bool) -> Vec<u8> {
    if color {
        layer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
    } else {
        layer.iter().flat_map(|&v| [v, v, v, 255]).collect()
    }
}

fn thumbnail(ctx: &egui::Context, rgba: &[u8], w: usize, h: usize) -> TextureHandle {
    let k = 132.0 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1));
    let small = resample::resize_rgba(rgba, w, h, nw, nh, Filter::Lanczos);
    ctx.load_texture("thumb", egui::ColorImage::from_rgba_unmultiplied([nw, nh], &small), egui::TextureOptions::LINEAR)
}

fn open_in_explorer(target: &Path) {
    let _ = std::process::Command::new("explorer").arg(target).spawn();
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
            .with_title("後付け影MOD")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("後付け影MOD", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
