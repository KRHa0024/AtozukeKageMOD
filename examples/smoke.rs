//! End-to-end check without the UI: start ComfyUI, verify the models, run one shading job, save the layers.
//!
//!     KAGE_DATA_DIR=<scratch> [KAGE_RUNS=2] cargo run --release --example smoke -- <comfy_dir> <model_dir> <image> <out_dir> [extract_shaded]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use kage::comfy::{Comfy, State};
use kage::log::Log;
use kage::models::Verifier;
use kage::pipeline::{self, Event, Params, Request};
use kage::shading::Image8;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (comfy_dir, model_dir, image, out) = (&args[0], PathBuf::from(&args[1]), &args[2], PathBuf::from(&args[3]));
    let data = kage::config::data_dir();
    let log = Log::new(&data.join("logs"));
    let verifier = Verifier::new(&data, log.clone());
    verifier.start(model_dir.clone());
    let comfy = Comfy::new(&data, log.clone());
    comfy.start(comfy_dir, &model_dir);
    let mut printed = 0;
    let mut flush = |log: &Log| {
        let (_, lines) = log.snapshot();
        for l in &lines[printed..] {
            println!("{}  {:<5} {}", l.time, l.level.tag(), l.text);
        }
        printed = lines.len();
    };
    while verifier.running() || comfy.state() == State::Starting {
        std::thread::sleep(Duration::from_millis(300));
        flush(&log);
    }
    println!("models {:?}  comfy {:?}", verifier.statuses(), comfy.state());
    let original = Arc::new(Image8::from_dynamic(&image::open(image).unwrap()));
    let shaded = args.get(4).map(|p| Arc::new(Image8::from_dynamic(&image::open(p).unwrap())));
    let runs: u32 = std::env::var("KAGE_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    for run in 1..=runs {
    let (tx, rx) = mpsc::channel();
    pipeline::spawn(
        Request {
            original: original.clone(),
            name: image.clone(),
            params: Params { light_dir: 0, extra: String::new(), strength: 1.0, background: false, gray: false, ghost: false, seed: 42 + run as u64 },
            shaded: shaded.clone(),
            model_dir: model_dir.clone(),
            keep_loaded: run < runs,
        },
        pipeline::Ctx { comfy: comfy.clone(), verifier: verifier.clone(), log: log.clone(), cancel: Arc::new(AtomicBool::new(false)), tx, notify: Arc::new(|| {}) },
    );
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Event::Steps(v, m)) => {
                if v == 1 || v == m {
                    println!("step {v}/{m}");
                }
            }
            Ok(Event::Stage(s)) => println!("stage {s}"),
            Ok(Event::WaitingForComfy) => println!("waiting for ComfyUI"),
            Ok(Event::Done(r)) => {
                std::fs::create_dir_all(&out).unwrap();
                let l = &r.layers;
                let ct = if l.color { image::ColorType::Rgb8 } else { image::ColorType::L8 };
                image::save_buffer(out.join("multiply.png"), &l.multiply, l.w as u32, l.h as u32, ct).unwrap();
                image::save_buffer(out.join("preview.png"), &l.preview, l.w as u32, l.h as u32, image::ColorType::Rgba8).unwrap();
                image::save_buffer(out.join("ai.png"), &r.shaded.rgba, r.shaded.w as u32, r.shaded.h as u32, image::ColorType::Rgba8).unwrap();
                if let Some(m) = &r.mask {
                    image::save_buffer(out.join("mask.png"), &m.0, m.1 as u32, m.2 as u32, image::ColorType::L8).unwrap();
                }
                flush(&log);
                println!("run {run}: done in {:.1} s -> {}", r.seconds, out.display());
                break;
            }
            Ok(Event::Failed(e)) => {
                flush(&log);
                println!("FAILED: {e}");
                break;
            }
            Ok(Event::Cancelled) => break,
            Err(_) => flush(&log),
        }
    }
    }
    comfy.stop();
}
