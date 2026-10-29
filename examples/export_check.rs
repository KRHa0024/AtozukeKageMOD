//! Writes every export format from the regression fixtures, for checking with other readers (Pillow, Photoshop).
//!
//!     cargo run --release --example export_check -- <out_dir>

use kage::blend::{self, Mode};
use kage::export::{self, Format, Pixels, PsdLayer};
use kage::shading::{self, Image8, Options};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let original = Image8::from_dynamic(&image::open(fx.join("original.png")).unwrap());
    let shaded = Image8::from_dynamic(&image::open(fx.join("shaded.png")).unwrap());
    let l = shading::shadow_layers(&original, &shaded, None, &Options { color: true, ..Options::default() });
    let (w, h) = (l.w, l.h);
    let composite = blend::composite(&original.rgba, &l.multiply, 3, Mode::Multiply, 1.0);
    // Opaque pixels: multiply at 100% is the preview up to the 8-bit rounding of the layer (as in Photoshop);
    // transparent ones keep their own color instead of white.
    for (c, p) in composite.chunks_exact(4).zip(l.preview.chunks_exact(4)).filter(|(c, _)| c[3] == 255) {
        assert!(c.iter().zip(p).all(|(a, b)| a.abs_diff(*b) <= 1), "multiply at 100% must match the preview: {c:?} {p:?}");
    }
    for f in [Format::Png, Format::Jpeg, Format::Webp] {
        export::save(&Pixels { w, h, channels: 3, data: &l.multiply }, &out.join(format!("shadow.{}", f.ext())), f, 6, 92).unwrap();
        export::save(&Pixels { w, h, channels: 4, data: &composite }, &out.join(format!("shaded.{}", f.ext())), f, 6, 92).unwrap();
    }
    let mul_rgba: Vec<u8> = l.multiply.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    let ai = export::to_canvas_rgba(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, w, h);
    let layers = [
        PsdLayer { name: "Original", rgba: original.rgba.clone(), mode: None, opacity: 1.0, visible: true },
        PsdLayer { name: "AI の出力", rgba: ai, mode: None, opacity: 1.0, visible: false },
        PsdLayer { name: "乗算レイヤー", rgba: mul_rgba, mode: Some(Mode::Multiply), opacity: 0.8, visible: true },
    ];
    export::save_psd(&out.join("layers.psd"), w, h, &layers, &composite).unwrap();
    println!("wrote {}", out.display());
}
