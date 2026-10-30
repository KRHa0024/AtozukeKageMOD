//! Writes every export format through the app's export job from the regression fixtures, for checking with other
//! readers (Pillow).
//!
//!     cargo run --release --example export_check -- <out_dir>

use kage::blend::{Mode, View};
use kage::export::{self, Encoding, Format, Job, Pixels};
use kage::shading::{self, Image8, Options};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let original = Image8::from_dynamic(&image::open(fx.join("original.png")).unwrap());
    let shaded = Image8::from_dynamic(&image::open(fx.join("shaded.png")).unwrap());
    let l = shading::shadow_layers(&original, &shaded, None, &Options { color: true, ..Options::default() });
    let ai_full = export::to_canvas_rgba(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, l.w, l.h);
    for format in [Format::Png, Format::Jpeg, Format::Webp] {
        let job = Job {
            original: &original,
            shaded: &shaded,
            ai_full: &ai_full,
            layers: &l,
            view: View { visible: [true, false, true], opacity: [0.8, 1.0, 1.0], mode: Mode::Multiply },
            targets: [true, true, true],
            encoding: Encoding { format, png_level: 6, quality: 92 },
            dir: &out,
            stem: "check",
        };
        let outcome = export::run_job(&job);
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        println!("{format:?}: {:?}", outcome.written);
    }
}
