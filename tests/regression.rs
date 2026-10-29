//! The Rust shadow layer must match the Python reference (orchestrator shading.py) on the fixtures made by
//! tests/gen_fixtures.py. Set KAGE_FIXTURES=<dir> to run against another fixture set (e.g. real images).

use std::path::PathBuf;

use kage::shading::{self, Image8, Options};

fn dir() -> PathBuf {
    std::env::var_os("KAGE_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
}

fn load(name: &str) -> image::DynamicImage {
    image::open(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// (mean abs diff, max abs diff, fraction of values that differ) in 0-255 units.
fn diff(a: &[u8], b: &[u8]) -> (f64, u8, f64) {
    assert_eq!(a.len(), b.len(), "size mismatch");
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut n = 0usize;
    for (&x, &y) in a.iter().zip(b) {
        let d = x.abs_diff(y);
        sum += d as u64;
        max = max.max(d);
        n += (d != 0) as usize;
    }
    (sum as f64 / a.len() as f64, max, n as f64 / a.len() as f64)
}

fn check(what: &str, ours: &[u8], theirs: &[u8], mean_tol: f64, max_tol: u8) {
    let (mean, max, frac) = diff(ours, theirs);
    println!("{what:<42} mean {mean:.4}  max {max}  differing {:.3}%", frac * 100.0);
    assert!(mean <= mean_tol && max <= max_tol, "{what}: mean {mean} max {max}");
}

#[test]
fn mask_blur_matches_pillow() {
    let src = load("mask_src.png").to_luma8();
    let (w, h) = (src.width() as usize, src.height() as usize);
    let ours = kage::resample::gaussian_blur_l(src.as_raw(), w, h, 1.5);
    check("mask GaussianBlur(1.5)", &ours, load("mask_blurred.png").to_luma8().as_raw(), 0.0, 0);
}

#[test]
fn lanczos_matches_pillow() {
    // gen_fixtures.py writes shaded.png = shaded_full.png resized with Pillow's LANCZOS.
    let full = load("shaded_full.png").to_rgb8();
    let small = load("shaded.png").to_rgb8();
    let ours = kage::resample::resize_u8(
        full.as_raw(),
        full.width() as usize,
        full.height() as usize,
        3,
        small.width() as usize,
        small.height() as usize,
        kage::resample::Filter::Lanczos,
    );
    check("Lanczos downscale", &ours, small.as_raw(), 0.0, 0);
}

#[test]
fn shadow_layers_match_python() {
    let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir().join("cases.json")).unwrap()).unwrap();
    let original = Image8::from_dynamic(&load("original.png"));
    let mask = load("mask_blurred.png").to_luma8();
    let (mw, mh) = (mask.width() as usize, mask.height() as usize);
    for (name, case) in cases.as_object().unwrap() {
        let kw = &case["kwargs"];
        let opt = Options {
            color: kw["color"].as_bool().unwrap_or(false),
            smooth: kw["smooth"].as_f64().unwrap_or(0.0) as f32,
            strength: kw.get("strength").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            ..Options::default()
        };
        let shaded = Image8::from_dynamic(&load(if case["same_size"].as_bool().unwrap() { "shaded_full.png" } else { "shaded.png" }));
        let m = case["mask"].as_bool().unwrap().then(|| (mask.as_raw().as_slice(), mw, mh));
        let layers = shading::shadow_layers(&original, &shaded, m, &opt);

        let expect_mul = load(&format!("{name}_multiply.png"));
        let expect_mul = if layers.color { expect_mul.to_rgb8().into_raw() } else { expect_mul.to_luma8().into_raw() };
        check(&format!("{name}: multiply"), &layers.multiply, &expect_mul, 0.005, 1);
        let alpha: Vec<u8> = layers.overlay.chunks_exact(4).map(|p| p[3]).collect();
        check(&format!("{name}: overlay alpha"), &alpha, load(&format!("{name}_overlay_alpha.png")).to_luma8().as_raw(), 0.005, 1);
        let preview = load(&format!("{name}_preview.png"));
        let ours_preview: Vec<u8> = if preview.color().has_alpha() {
            layers.preview.clone()
        } else {
            layers.preview.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
        };
        let theirs = if preview.color().has_alpha() { preview.to_rgba8().into_raw() } else { preview.to_rgb8().into_raw() };
        check(&format!("{name}: preview"), &ours_preview, &theirs, 0.005, 1);

        let err = case["reconstruction_error"].as_f64().unwrap();
        println!("{name:<42} reconstruction error {:.3} (python {err:.3})", layers.stats.reconstruction_error);
        assert!((layers.stats.reconstruction_error - err).abs() < 0.05);
    }
}
