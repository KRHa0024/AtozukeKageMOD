//! The application state; each screen part lives in its own module as an `impl App` block.

mod canvas;
mod controls;
mod dialogs;
mod logic;
mod panel;
mod settings;
mod shot;
mod status;
mod views;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{self, Frame, Margin, Stroke, TextureHandle, Vec2};
use kage::blend::{View, MODES, SHADOW};
use kage::comfy::{Comfy, Failure};
use kage::config::{self, Config};
use kage::export::Outcome;
use kage::log::Log;
use kage::models::Verifier;
use kage::pipeline::{Event, Params, RunResult, Solved};
use kage::shading::Image8;

use crate::i18n::{t, Lang, T};
use crate::theme::{self, BG, CANVAS_SHADES, LINE, PANEL};
use shot::Shot;
use views::Views;

/// Indexes of the settings tabs (`T::tabs`).
const MODELS_TAB: usize = 0;
const COMFY_TAB: usize = 1;

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Main,
    Settings,
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Running { started: f64 },
    Done,
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
    /// The result, for the history list.
    thumb: TextureHandle,
    /// Layer panel thumbnails (shadow, AI output, original).
    layer_thumbs: [TextureHandle; 3],
    /// Written out since the last change, so quitting loses nothing.
    exported: bool,
}

struct Resolve {
    id: u64,
    params: Params,
    rx: Receiver<Solved>,
}

struct Toast {
    ok: bool,
    text: String,
    at: f64,
    folder: Option<PathBuf>,
}

pub struct App {
    lang: Lang,
    screen: Screen,
    settings_tab: usize,
    data: PathBuf,
    cfg: Config,
    saved_cfg: Config,
    cfg_changed_at: f64,
    applied_dirs: (String, String),
    /// A directory changed during a run: applied when the run ends.
    dirs_pending: bool,
    log: Log,
    verifier: Verifier,
    comfy: Comfy,

    image: Option<Input>,
    seed: String,

    phase: Phase,
    stage: usize,
    waiting_comfy: bool,
    steps: (u32, u32),
    first_step_at: Option<f64>,
    last_step_at: f64,
    run_rx: Option<Receiver<Event>>,
    run_input: Option<Input>,
    /// The run only adds the subject mask to this history entry.
    mask_run: Option<u64>,
    cancel: Arc<AtomicBool>,
    error: Option<Failure>,

    history: Vec<Entry>,
    next_id: u64,
    current: Option<u64>,
    views: Option<Views>,
    resolve: Option<Resolve>,
    /// Strength, gray, double lines and background as last seen, to time the re-solve.
    post_seen: (f32, bool, bool, bool),
    post_changed_at: f64,

    visible: [bool; 3],
    selected_layer: usize,
    /// Per layer; the original's stays 1.
    opacity: [f32; 3],
    /// The layer shown alone by Alt+click, and the visibility to go back to.
    solo: Option<(usize, [bool; 3])>,
    compare_on: bool,
    compare: f32,
    /// Hold C: the original over everything.
    peek_original: bool,
    zoom: Option<f32>,
    /// Points per image pixel, as drawn last.
    scale: f32,
    pan: Vec2,
    /// Magnified far enough to show single pixels.
    nearest: bool,
    input_filter: Option<(egui::TextureId, bool)>,
    /// Right panel: 0 layers, 1 history.
    right_tab: usize,
    export_open: bool,
    export_pending: bool,
    export_rx: Option<Receiver<(u64, Outcome)>>,
    last_export_dir: Option<PathBuf>,
    toast: Option<Toast>,
    quit_open: bool,
    quit_confirmed: bool,
    window_size: Option<[f32; 2]>,
    log_level: usize,
    log_seen: u64,
    adv_default_open: bool,
    shot: Option<Shot>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::setup_fonts(&cc.egui_ctx);
        theme::setup_style(&cc.egui_ctx);
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
            settings_tab: COMFY_TAB,
            applied_dirs: (cfg.model_dir.clone(), cfg.comfy_dir.clone()),
            dirs_pending: false,
            post_seen: (cfg.strength, cfg.gray, cfg.ghost, cfg.bg_shadow),
            saved_cfg: cfg.clone(),
            cfg,
            cfg_changed_at: 0.0,
            data,
            log,
            verifier,
            comfy,
            image: None,
            seed: "random".into(),
            phase: Phase::Idle,
            stage: 0,
            waiting_comfy: false,
            steps: (0, 0),
            first_step_at: None,
            last_step_at: 0.0,
            run_rx: None,
            run_input: None,
            mask_run: None,
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
            solo: None,
            compare_on: false,
            compare: 0.5,
            peek_original: false,
            zoom: None,
            scale: 1.0,
            pan: Vec2::ZERO,
            nearest: false,
            input_filter: None,
            right_tab: 0,
            export_open: false,
            export_pending: false,
            export_rx: None,
            last_export_dir: None,
            toast: None,
            quit_open: false,
            quit_confirmed: false,
            window_size: None,
            log_level: 0,
            log_seen: 0,
            adv_default_open: false,
            shot: None,
        }
        .with_scene(&cc.egui_ctx)
    }

    fn t(&self) -> &'static T {
        t(self.lang)
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    fn view(&self) -> View {
        View { visible: self.visible, opacity: self.opacity, mode: MODES[self.cfg.blend] }
    }

    fn entry(&self, id: Option<u64>) -> Option<&Entry> {
        id.and_then(|id| self.history.iter().find(|e| e.id == id))
    }

    fn lang_code(&self) -> &'static str {
        if self.lang == Lang::En {
            "en"
        } else {
            "ja"
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.quit_confirmed || self.shot.is_some() {
            return;
        }
        if self.history.iter().any(|e| !e.exported) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_open = true;
        }
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
        self.window_size = ctx.input(|i| i.viewport().inner_rect).map(|r| [r.width(), r.height()]);
        self.handle_close(&ctx);
        self.shortcuts(&ctx);
        self.tick(&ctx);
        self.handle_shot(&ctx);
        if self.verifier.running() || self.comfy.state() == kage::comfy::State::Starting {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        let bar = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(6, 4)).stroke(Stroke::new(1.0, LINE));
        let side = Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, LINE));
        let padded = Frame::new().fill(PANEL).inner_margin(Margin::same(8)).stroke(Stroke::new(1.0, LINE));
        match self.screen {
            Screen::Main => {
                egui::Panel::top("toolbar").frame(bar).exact_size(36.0).show(ui, |ui| self.toolbar(ui));
                egui::Panel::bottom("status").frame(bar).exact_size(40.0).show(ui, |ui| self.status_bar(ui));
                egui::Panel::left("controls").frame(side).exact_size(270.0).resizable(false).show_separator_line(false).show(ui, |ui| self.controls(ui));
                self.ensure_views(&ctx);
                egui::Panel::right("right").frame(padded).exact_size(280.0).resizable(false).show_separator_line(false).show(ui, |ui| self.right_panel(ui));
                egui::Panel::bottom("view_tools").frame(bar).exact_size(36.0).show(ui, |ui| self.view_tools(ui));
                let canvas = Frame::new().fill(CANVAS_SHADES[self.cfg.canvas_shade]);
                egui::CentralPanel::default().frame(canvas).show(ui, |ui| self.canvas(ui));
                if self.export_open {
                    self.export_modal(&ctx);
                }
            }
            Screen::Settings => {
                egui::Panel::top("settings_toolbar").frame(bar).exact_size(36.0).show(ui, |ui| self.settings_toolbar(ui));
                egui::Panel::bottom("status").frame(bar).exact_size(40.0).show(ui, |ui| self.status_bar(ui));
                egui::Panel::left("settings_nav").frame(padded).exact_size(180.0).resizable(false).show_separator_line(false).show(ui, |ui| self.settings_nav(ui));
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(12))).show(ui, |ui| self.settings_body(ui));
            }
        }
        if self.quit_open {
            self.quit_modal(&ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.store(true, Ordering::SeqCst);
        self.cfg.lang = self.lang_code().into();
        self.cfg.window = self.window_size.or(self.cfg.window);
        self.save_config();
        self.comfy.stop();
    }
}
