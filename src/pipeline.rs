//! One shading run: AI shading (Qwen-Image 2.1 instruction edit) -> subject mask (BiRefNet) -> shadow layer.
//!
//! The ComfyUI graphs (Comfy-Org templates image_qwen_image_2_1 edit and utility_birefnet_remove_background, both
//! core nodes only) and the parameters are the ones the shading was tuned with; keep them unchanged.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use crate::comfy::{self, Comfy, RunError};
use crate::log::Log;
use crate::models::{Verifier, MODELS};
use crate::shading::{self, Image8, Layers, Options};

/// Tuned on VRChat / anime renders: asking for "natural, only darken" made the model grade the whole image (pink
/// hair turned brown); illustration-style shading keeps colors and gives defined shadow shapes.
const SHADE_PROMPT: &str = "Add strong, beautiful anime-style shading to the characters, like a professionally shaded illustration{light}: crisp shadow shapes under the bangs and on the neck, under the chin, on the inner sides of the arms and legs, in clothing folds and where parts overlap, with soft gradients on the turned-away side. Highlights and lit areas stay exactly as in the original. Keep all colors, the face, every detail, the background and the framing exactly the same; only add shadows.";
/// Sources larger than this are shaded at a higher model resolution (finer shadow shapes for 4K renders).
const LARGE_SOURCE_PIXELS: usize = 2_500_000;
const LARGE_RESOLUTION: u32 = 1536;
const DEFAULT_RESOLUTION: u32 = 1024;
const STEPS: u32 = 25;

/// Light direction cells (3x3 grid, index 4 = match the image's own light).
const LIGHT_FROM: [&str; 9] =
    ["the upper left", "above", "the upper right", "the left", "", "the right", "the lower left", "below", "the lower right"];
pub const AUTO_LIGHT: usize = 4;

#[derive(Clone, Debug)]
pub struct Params {
    pub light_dir: usize,
    pub extra: String,
    pub strength: f32,
    /// Shade the background too (no subject mask).
    pub background: bool,
    pub gray: bool,
    /// Remove double lines (smoothing 1.0).
    pub ghost: bool,
    pub seed: u64,
}

impl Params {
    pub fn options(&self) -> Options {
        Options { color: !self.gray, strength: self.strength, smooth: if self.ghost { 1.0 } else { 0.0 }, ..Options::default() }
    }

    fn lighting(&self) -> String {
        let extra = self.extra.trim();
        let from = LIGHT_FROM.get(self.light_dir).copied().unwrap_or("");
        match (extra.is_empty(), from.is_empty()) {
            (true, true) => String::new(),
            (true, false) => format!("light coming from {from}"),
            (false, true) => extra.to_string(),
            (false, false) => format!("{extra} coming from {from}"),
        }
    }

    pub fn prompt(&self) -> String {
        let light = self.lighting();
        SHADE_PROMPT.replace("{light}", &if light.is_empty() { String::new() } else { format!(", lit by {light}") })
    }
}

pub struct Request {
    pub original: Arc<Image8>,
    pub name: String,
    pub params: Params,
    /// Extract only: an already shaded version is given, the AI is not run.
    pub shaded: Option<Arc<Image8>>,
    pub model_dir: PathBuf,
    pub keep_loaded: bool,
}

pub struct RunResult {
    pub original: Arc<Image8>,
    pub name: String,
    pub params: Params,
    pub extracted: bool,
    /// AI output (model resolution) or the given shaded image.
    pub shaded: Arc<Image8>,
    /// Blurred subject mask (None when the background is shaded too).
    pub mask: Option<Arc<(Vec<u8>, usize, usize)>>,
    pub layers: Layers,
    pub seconds: f32,
}

pub enum Event {
    WaitingForComfy,
    /// 0 AI shading, 1 subject, 2 shadow layer, 3 finishing.
    Stage(usize),
    Steps(u32, u32),
    Done(Box<RunResult>),
    Failed(String),
    Cancelled,
}

pub struct Ctx {
    pub comfy: Comfy,
    pub verifier: Verifier,
    pub log: Log,
    pub cancel: Arc<AtomicBool>,
    pub tx: Sender<Event>,
    pub notify: Arc<dyn Fn() + Send + Sync>,
}

impl Ctx {
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        (self.notify)();
    }
}

pub fn spawn(req: Request, ctx: Ctx) {
    std::thread::spawn(move || {
        let started = Instant::now();
        match run(&req, &ctx, started) {
            Ok(result) => ctx.send(Event::Done(Box::new(result))),
            Err(RunError::Cancelled) => {
                ctx.log.info("Stopped by the user");
                ctx.send(Event::Cancelled)
            }
            Err(RunError::Failed(msg)) => {
                ctx.log.error(format!("Run failed: {msg}"));
                ctx.send(Event::Failed(msg))
            }
        }
        if !req.keep_loaded {
            ctx.comfy.free();
        }
    });
}

fn cancelled(ctx: &Ctx) -> Result<(), RunError> {
    if ctx.cancel.load(std::sync::atomic::Ordering::SeqCst) {
        Err(RunError::Cancelled)
    } else {
        Ok(())
    }
}

fn wait_for_comfy(ctx: &Ctx) -> Result<(), RunError> {
    let mut announced = false;
    loop {
        cancelled(ctx)?;
        match ctx.comfy.state() {
            comfy::State::Ready { .. } => return Ok(()),
            comfy::State::Starting => {
                if !announced {
                    ctx.send(Event::WaitingForComfy);
                    announced = true;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            comfy::State::NotSet | comfy::State::NotFound => {
                return Err(RunError::Failed("ComfyUI is not set up (Settings > ComfyUI)".into()))
            }
            comfy::State::MissingNodes(n) => {
                return Err(RunError::Failed(format!("ComfyUI is too old (missing {}); update it", n.join(", "))))
            }
            comfy::State::Failed(e) => return Err(RunError::Failed(format!("ComfyUI is not running: {e}"))),
        }
    }
}

fn run(req: &Request, ctx: &Ctx, started: Instant) -> Result<RunResult, RunError> {
    let p = &req.params;
    let (w, h) = (req.original.w, req.original.h);
    let needs_comfy = req.shaded.is_none() || !p.background;
    if needs_comfy {
        if !ctx.verifier.still_valid(&req.model_dir) {
            return Err(RunError::Failed("the model files are missing or not verified (Settings > Models)".into()));
        }
        wait_for_comfy(ctx)?;
    }
    let input_name = format!("kage_{}_{:08x}.png", crate::log::stamp(), comfy::random_u64() as u32);
    let input_path = ctx.comfy.input_dir().join(&input_name);
    if needs_comfy {
        write_png(&req.original, &input_path).map_err(|e| RunError::Failed(format!("could not write the input: {e}")))?;
    }
    let _cleanup = Cleanup(vec![input_path.clone()]);

    let shaded = match &req.shaded {
        Some(s) => {
            ctx.log.info(format!("Extracting from a given shaded image {}x{} (no AI)", s.w, s.h));
            s.clone()
        }
        None => {
            ctx.send(Event::Stage(0));
            let resolution = if w * h > LARGE_SOURCE_PIXELS { LARGE_RESOLUTION } else { DEFAULT_RESOLUTION };
            ctx.log.info(format!("Input {} {w}x{h}; model resolution {resolution}", req.name));
            ctx.log.info(format!(
                "Shading started: light={} strength={} scope={} color={} smooth={} seed={}",
                if p.light_dir == AUTO_LIGHT { "auto".to_string() } else { LIGHT_FROM[p.light_dir].replace(' ', "-") },
                p.strength,
                if p.background { "all" } else { "subject" },
                !p.gray,
                if p.ghost { 1.0 } else { 0.0 },
                p.seed
            ));
            if !p.extra.trim().is_empty() {
                ctx.log.info(format!("Extra prompt: {}", p.extra.trim()));
            }
            let t0 = Instant::now();
            let mut first_step: Option<f32> = None;
            let files = ctx.comfy.run(&shade_graph(&input_name, &p.prompt(), resolution, p.seed), "9", &ctx.cancel, |v, m| {
                if first_step.is_none() {
                    first_step = Some(t0.elapsed().as_secs_f32());
                }
                ctx.send(Event::Steps(v, m));
            })?;
            ctx.log.info(format!(
                "AI: {STEPS}/{STEPS} steps done in {:.1} s (first step after {:.1} s)",
                t0.elapsed().as_secs_f32(),
                first_step.unwrap_or(0.0)
            ));
            let img = load_output(&files)?;
            Arc::new(img)
        }
    };
    cancelled(ctx)?;

    let mask = if p.background {
        None
    } else {
        ctx.send(Event::Stage(1));
        let t0 = Instant::now();
        let files = ctx.comfy.run(&mask_graph(&input_name), "9", &ctx.cancel, |_, _| {})?;
        let cutout = image::open(&files[0]).map_err(|e| RunError::Failed(format!("subject mask: {e}")))?;
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
        ctx.log.info(format!("Subject mask (BiRefNet) in {:.1} s", t0.elapsed().as_secs_f32()));
        Some(Arc::new(shading::subject_mask(&cutout)))
    };
    cancelled(ctx)?;

    ctx.send(Event::Stage(2));
    let layers = solve(&req.original, &shaded, mask.as_deref(), p, &ctx.log);
    cancelled(ctx)?;
    ctx.send(Event::Stage(3));
    let seconds = started.elapsed().as_secs_f32();
    ctx.log.info(format!("Done in {seconds:.1} s"));
    Ok(RunResult {
        original: req.original.clone(),
        name: req.name.clone(),
        params: p.clone(),
        extracted: req.shaded.is_some(),
        shaded,
        mask,
        layers,
        seconds,
    })
}

/// Solve (or re-solve, after the strength / color / smoothing changed) the shadow layer.
pub fn solve(original: &Image8, shaded: &Image8, mask: Option<&(Vec<u8>, usize, usize)>, p: &Params, log: &Log) -> Layers {
    let t0 = Instant::now();
    let m = mask.map(|(px, w, h)| (px.as_slice(), *w, *h));
    let layers = shading::shadow_layers(original, shaded, m, &p.options());
    let s = &layers.stats;
    if s.brightened_fraction > 0.02 {
        log.warn(format!(
            "{:.1}% of pixels got brighter; they are not included in the shadow layer",
            s.brightened_fraction * 100.0
        ));
    }
    log.info(format!(
        "Shadow layer {}x{} -> {}x{} in {:.1} s (darkening {:.3}, error {:.2})",
        s.solve_size.0,
        s.solve_size.1,
        layers.w,
        layers.h,
        t0.elapsed().as_secs_f32(),
        s.mean_darkening,
        s.reconstruction_error
    ));
    layers
}

/// Qwen-Image 2.1 instruction edit (workflows/edit/qwen_image_2_1_edit.json without the unused reference images).
fn shade_graph(image: &str, prompt: &str, resolution: u32, seed: u64) -> Value {
    json!({
        "1": {"class_type": "UNETLoader", "inputs": {"unet_name": MODELS[0].file, "weight_dtype": "default"}},
        "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": MODELS[1].file, "type": "qwen_image", "device": "default"}},
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": MODELS[2].file}},
        "4": {"class_type": "QwenImage21Cache", "inputs": {"model": ["1", 0], "device": "auto", "dtype": "default"}},
        "20": {"class_type": "LoadImage", "inputs": {"image": image}},
        "5": {"class_type": "TextEncodeQwenImage21", "inputs": {
            "clip": ["2", 0], "vae": ["3", 0], "prompt": prompt, "negative_prompt": "",
            "resolution": resolution, "images.image_1": ["20", 0]
        }},
        "7": {"class_type": "KSampler", "inputs": {
            "model": ["4", 0], "positive": ["5", 0], "negative": ["5", 1], "latent_image": ["5", 2],
            "seed": seed, "steps": STEPS, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple", "denoise": 1.0
        }},
        "8": {"class_type": "VAEDecode", "inputs": {"samples": ["7", 0], "vae": ["3", 0]}},
        "9": {"class_type": "SaveImage", "inputs": {"images": ["8", 0], "filename_prefix": "kage/shaded"}}
    })
}

/// BiRefNet background removal (workflows/utilities/remove_background_birefnet.json); alpha = subject.
fn mask_graph(image: &str) -> Value {
    json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": image}},
        "2": {"class_type": "LoadBackgroundRemovalModel", "inputs": {"bg_removal_name": MODELS[3].file}},
        "3": {"class_type": "RemoveBackground", "inputs": {"bg_removal_model": ["2", 0], "image": ["1", 0]}},
        "4": {"class_type": "InvertMask", "inputs": {"mask": ["3", 0]}},
        "5": {"class_type": "JoinImageWithAlpha", "inputs": {"image": ["1", 0], "alpha": ["4", 0]}},
        "9": {"class_type": "SaveImage", "inputs": {"images": ["5", 0], "filename_prefix": "kage/cutout"}}
    })
}

fn load_output(files: &[PathBuf]) -> Result<Image8, RunError> {
    let img = image::open(&files[0]).map_err(|e| RunError::Failed(format!("AI output: {e}")))?;
    for f in files {
        let _ = std::fs::remove_file(f);
    }
    Ok(Image8::from_dynamic(&img))
}

/// The original as PNG for ComfyUI's LoadImage (alpha kept, like passing the file as is).
fn write_png(img: &Image8, path: &Path) -> image::ImageResult<()> {
    use image::ImageEncoder;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        f,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    if img.has_alpha {
        enc.write_image(&img.rgba, img.w as u32, img.h as u32, image::ExtendedColorType::Rgba8)
    } else {
        let rgb: Vec<u8> = img.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        enc.write_image(&rgb, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8)
    }
}

struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}
