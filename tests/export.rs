//! Export: file targets and numbering, alpha and ICC handling per format, size limits and lossless WebP.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::ImageDecoder;
use kage::blend::{self, Mode, View, MODES};
use kage::export::{self, Encoding, Format, Job, Outcome, Pixels};
use kage::shading::{self, Image8, Layers, Options, Stats};

// ---------------------------------------------------------------------------------------------------------------
// Helpers

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("kage_test_export_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> image::DynamicImage {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    image::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn noise(len: usize, mut seed: u32) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as u8
        })
        .collect()
}

struct Case {
    original: Image8,
    shaded: Image8,
    ai_full: Vec<u8>,
    layers: Layers,
}

impl Case {
    /// From the regression fixtures (the original has transparent, semi-transparent and opaque pixels).
    fn fixtures(color: bool, opaque: bool) -> Self {
        let mut original = Image8::from_dynamic(&fixture("original.png"));
        if opaque {
            for p in original.rgba.chunks_exact_mut(4) {
                p[3] = 255;
            }
            original.has_alpha = false;
        }
        let shaded = Image8::from_dynamic(&fixture("shaded.png"));
        let layers = shading::shadow_layers(&original, &shaded, None, &Options { color, ..Options::default() });
        let ai_full = export::to_canvas_rgba(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, layers.w, layers.h);
        Self { original, shaded, ai_full, layers }
    }

    /// A flat image of any size (for the size limits).
    fn flat(w: usize, h: usize) -> Self {
        let n = w * h;
        let rgba: Vec<u8> = (0..n).flat_map(|i| [(i % 251) as u8, 100, 200, 255]).collect();
        let image = Image8 { w, h, has_alpha: false, rgba, icc: None, deep: false };
        let layers = Layers {
            w,
            h,
            multiply: vec![128; n],
            color: false,
            overlay: (0..n).flat_map(|_| [0, 0, 0, 127]).collect(),
            preview: vec![0; n * 4],
            stats: Stats::default(),
        };
        Self { shaded: image.clone(), ai_full: image.rgba.clone(), original: image, layers }
    }

    fn with_icc(mut self, icc: &[u8]) -> Self {
        self.original.icc = Some(Arc::new(icc.to_vec()));
        self
    }

    fn job<'a>(&'a self, dir: &'a Path, view: View, targets: [bool; 3], format: Format, quality: u8) -> Job<'a> {
        Job {
            original: &self.original,
            shaded: &self.shaded,
            ai_full: &self.ai_full,
            layers: &self.layers,
            view,
            targets,
            encoding: Encoding { format, png_level: 3, quality },
            dir,
            stem: "img",
        }
    }

    fn flatten(&self, view: &View) -> Vec<u8> {
        blend::flatten_view(&self.original, &self.ai_full, &self.layers, view)
    }
}

fn view(mode: Mode) -> View {
    View { visible: [true, false, true], opacity: [0.8, 1.0, 1.0], mode }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn names_of(out: &Outcome) -> Vec<String> {
    out.written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
}

fn icc_of(path: &Path) -> Option<Vec<u8>> {
    let reader = image::ImageReader::open(path).unwrap().with_guessed_format().unwrap();
    let mut decoder = reader.into_decoder().unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    decoder.icc_profile().unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fake_icc(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}


#[test]
fn only_the_selected_targets_are_written_with_the_planned_names() {
    let case = Case::fixtures(true, false);
    for format in [Format::Png, Format::Jpeg, Format::Webp] {
        for bits in 0..8u8 {
            let targets = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
            let dir = TempDir::new(&format!("targets_{}_{bits}", format.ext()));
            let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), targets, format, 90));
            assert!(out.error.is_none(), "{format:?} {targets:?}: {:?}", out.error);
            let planned = export::planned_names("img", targets, format);
            assert_eq!(names_of(&out), planned, "{format:?} {targets:?}");
            let mut sorted = planned.clone();
            sorted.sort();
            assert_eq!(entries(dir.path()), sorted, "{format:?} {targets:?}: files in the directory");
            for p in &out.written {
                assert!(p.starts_with(dir.path()) && p.is_file());
            }
        }
    }
}

#[test]
fn running_twice_numbers_the_files_instead_of_overwriting() {
    let case = Case::fixtures(true, false);
    let dir = TempDir::new("numbering");
    let sub = dir.path().join("nested").join("out"); // created on demand
    let mut first_bytes = Vec::new();
    for run in 1..=3 {
        let out = export::run_job(&case.job(&sub, view(Mode::Multiply), [true, true, true], Format::Png, 90));
        assert!(out.error.is_none(), "{:?}", out.error);
        let suffix = if run == 1 { String::new() } else { format!("_{run}") };
        assert_eq!(names_of(&out), [format!("img_shadow{suffix}.png"), format!("img_ai{suffix}.png"), format!("img_shaded{suffix}.png")]);
        if run == 1 {
            first_bytes = std::fs::read(&out.written[0]).unwrap();
        }
    }
    assert_eq!(entries(&sub).len(), 9);
    assert_eq!(std::fs::read(sub.join("img_shadow.png")).unwrap(), first_bytes, "the first file is untouched");
}

#[test]
fn unique_path_fills_the_first_free_number() {
    let dir = TempDir::new("unique");
    let d = dir.path();
    assert_eq!(export::unique_path(d, "a", "", "png"), d.join("a.png"));
    assert_eq!(export::unique_path(d, "a", "shadow", "png"), d.join("a_shadow.png"));
    std::fs::write(d.join("a.png"), b"x").unwrap();
    std::fs::write(d.join("a_3.png"), b"x").unwrap();
    assert_eq!(export::unique_path(d, "a", "", "png"), d.join("a_2.png"));
    std::fs::write(d.join("a_2.png"), b"x").unwrap();
    assert_eq!(export::unique_path(d, "a", "", "png"), d.join("a_4.png"));
}

// ---------------------------------------------------------------------------------------------------------------
// Alpha

#[test]
fn normal_mode_shadow_png_is_black_with_alpha() {
    for color in [false, true] {
        let case = Case::fixtures(color, true);
        let dir = TempDir::new(&format!("normal_{color}"));
        let out = export::run_job(&case.job(dir.path(), view(Mode::Normal), [true, false, false], Format::Png, 90));
        assert!(out.error.is_none(), "{:?}", out.error);
        let img = image::open(&out.written[0]).unwrap();
        assert!(img.color().has_alpha(), "color={color}: the Normal shadow needs an alpha channel");
        let rgba = img.to_rgba8().into_raw();
        assert!(rgba == blend::shadow_rgba(&case.layers, Mode::Normal, &case.original.rgba), "color={color}");
        assert!(rgba.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]));
    }
}

#[test]
fn multiply_shadow_of_an_opaque_source_has_no_alpha() {
    for color in [false, true] {
        let case = Case::fixtures(color, true);
        let dir = TempDir::new(&format!("multiply_opaque_{color}"));
        let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [true, false, false], Format::Png, 90));
        assert!(out.error.is_none(), "{:?}", out.error);
        let img = image::open(&out.written[0]).unwrap();
        assert!(!img.color().has_alpha(), "color={color}: {:?}", img.color());
        assert_eq!(img.color(), if color { image::ColorType::Rgb8 } else { image::ColorType::L8 });
        let raw = if color { img.to_rgb8().into_raw() } else { img.to_luma8().into_raw() };
        assert!(raw == case.layers.multiply, "color={color}");
    }
}

#[test]
fn multiply_shadow_of_a_transparent_source_keeps_the_source_alpha() {
    for color in [false, true] {
        let case = Case::fixtures(color, false);
        let dir = TempDir::new(&format!("multiply_alpha_{color}"));
        let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [true, false, false], Format::Png, 90));
        assert!(out.error.is_none(), "{:?}", out.error);
        let img = image::open(&out.written[0]).unwrap();
        assert!(img.color().has_alpha());
        let rgba = img.to_rgba8().into_raw();
        let alpha: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
        let source: Vec<u8> = case.original.rgba.chunks_exact(4).map(|p| p[3]).collect();
        assert!(alpha == source, "color={color}: alpha");
        assert!(rgba == blend::shadow_rgba(&case.layers, Mode::Multiply, &case.original.rgba), "color={color}");
    }
}

#[test]
fn composite_png_of_a_transparent_source_keeps_its_alpha() {
    let case = Case::fixtures(true, false);
    for mode in MODES {
        let dir = TempDir::new("composite_alpha");
        let v = View { visible: [true, false, true], opacity: [0.7, 1.0, 1.0], mode };
        let out = export::run_job(&case.job(dir.path(), v, [false, false, true], Format::Png, 90));
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(names_of(&out), ["img_shaded.png"]);
        let rgba = image::open(&out.written[0]).unwrap().to_rgba8().into_raw();
        let alpha: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
        let source: Vec<u8> = case.original.rgba.chunks_exact(4).map(|p| p[3]).collect();
        assert!(alpha == source, "{mode:?}: alpha changed");
        assert!(rgba == case.flatten(&v), "{mode:?}: pixels differ from flatten_view");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ICC

#[test]
fn icc_profile_is_embedded_in_composite_and_ai_but_not_in_the_shadow() {
    let icc = fake_icc(200);
    for (opaque, alpha_label) in [(true, "opaque"), (false, "transparent")] {
        let case = Case::fixtures(true, opaque).with_icc(&icc);
        for format in [Format::Png, Format::Jpeg, Format::Webp] {
            let dir = TempDir::new(&format!("icc_{}_{alpha_label}", format.ext()));
            let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [true, true, true], format, 90));
            assert!(out.error.is_none(), "{format:?} {alpha_label}: {:?}", out.error);
            assert_eq!(out.written.len(), 3);
            let what = format!("{format:?} {alpha_label}");
            assert_eq!(icc_of(&out.written[0]), None, "{what}: shadow layer has no profile");
            assert_eq!(icc_of(&out.written[1]).as_deref(), Some(icc.as_slice()), "{what}: AI output");
            assert_eq!(icc_of(&out.written[2]).as_deref(), Some(icc.as_slice()), "{what}: composite");
            if format == Format::Webp && !opaque {
                assert!(image::open(&out.written[2]).unwrap().color().has_alpha(), "{what}: alpha survives the ICC wrapper");
            }
        }
    }
}

#[test]
fn without_a_profile_nothing_is_embedded() {
    let case = Case::fixtures(true, true);
    for format in [Format::Png, Format::Jpeg, Format::Webp] {
        let dir = TempDir::new(&format!("icc_none_{}", format.ext()));
        let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [true, true, true], format, 90));
        assert!(out.error.is_none());
        for p in &out.written {
            assert_eq!(icc_of(p), None, "{format:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Limits

#[test]
fn webp_over_16383_px_is_an_error_and_leaves_nothing_behind() {
    for (i, targets) in [[true, false, false], [false, true, false], [false, false, true], [true, true, true]].into_iter().enumerate() {
        let dir = TempDir::new(&format!("webp_big_{i}"));
        let case = Case::flat(16384, 2);
        let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), targets, Format::Webp, 90));
        assert!(out.error.is_some(), "{targets:?}: expected an error");
        assert!(out.written.is_empty(), "{targets:?}: {:?}", out.written);
        assert!(entries(dir.path()).is_empty(), "{targets:?}: left {:?}", entries(dir.path()));
    }
}

#[test]
fn webp_16383_px_works() {
    let dir = TempDir::new("webp_max");
    let case = Case::flat(16383, 2);
    let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [true, true, true], Format::Webp, 90));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.written.len(), 3);
    for p in &out.written {
        let img = image::open(p).unwrap();
        assert_eq!((img.width(), img.height()), (16383, 2), "{}", p.display());
    }
    assert_eq!(entries(dir.path()).len(), 3, "no .part files");
}

#[test]
fn jpeg_over_65535_px_is_an_error_and_leaves_nothing_behind() {
    for (i, targets) in [[true, false, false], [false, true, false], [false, false, true]].into_iter().enumerate() {
        let dir = TempDir::new(&format!("jpeg_big_{i}"));
        let case = Case::flat(70000, 2);
        let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), targets, Format::Jpeg, 90));
        assert!(out.error.is_some(), "{targets:?}: expected an error");
        assert!(out.written.is_empty());
        assert!(entries(dir.path()).is_empty(), "{targets:?}: left {:?}", entries(dir.path()));
    }
}

#[test]
fn jpeg_65535_px_works() {
    let dir = TempDir::new("jpeg_max");
    let case = Case::flat(65535, 2);
    let out = export::run_job(&case.job(dir.path(), view(Mode::Multiply), [false, false, true], Format::Jpeg, 90));
    assert!(out.error.is_none(), "{:?}", out.error);
    let img = image::open(&out.written[0]).unwrap();
    assert_eq!((img.width(), img.height()), (65535, 2));
}

// ---------------------------------------------------------------------------------------------------------------
// Lossless WebP

fn webp_enc(quality: u8) -> Encoding {
    Encoding { format: Format::Webp, png_level: 3, quality }
}

#[test]
fn webp_quality_100_is_lossless() {
    let (w, h) = (37usize, 21usize);
    // RGB
    let rgb = noise(w * h * 3, 7);
    let bytes = export::encode(&Pixels { w, h, channels: 3, data: &rgb }, &webp_enc(100), None).unwrap();
    assert!(image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgb8().into_raw() == rgb);
    // Gray pixels come back as gray RGB.
    let gray = noise(w * h, 8);
    let bytes = export::encode(&Pixels { w, h, channels: 1, data: &gray }, &webp_enc(100), None).unwrap();
    let expect: Vec<u8> = gray.iter().flat_map(|&v| [v, v, v]).collect();
    assert!(image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgb8().into_raw() == expect);
    // RGBA that is opaque everywhere.
    let mut rgba = noise(w * h * 4, 9);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    let bytes = export::encode(&Pixels { w, h, channels: 4, data: &rgba }, &webp_enc(100), None).unwrap();
    assert!(image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgba8().into_raw() == rgba);
    // With ICC wrapper too.
    let icc = fake_icc(131);
    let bytes = export::encode(&Pixels { w, h, channels: 3, data: &rgb }, &webp_enc(100), Some(&icc)).unwrap();
    assert!(image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgb8().into_raw() == rgb);
}

#[test]
fn webp_quality_100_keeps_alpha_and_visible_colours_exactly() {
    let (w, h) = (37usize, 21usize);
    let mut rgba = noise(w * h * 4, 10);
    for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
        p[3] = match i % 3 {
            0 => 0,
            1 => 255,
            _ => p[3].max(1),
        };
    }
    for icc in [None, Some(fake_icc(130))] {
        let bytes = export::encode(&Pixels { w, h, channels: 4, data: &rgba }, &webp_enc(100), icc.as_deref()).unwrap();
        let got = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap().to_rgba8().into_raw();
        for (i, (g, e)) in got.chunks_exact(4).zip(rgba.chunks_exact(4)).enumerate() {
            assert_eq!(g[3], e[3], "pixel {i}: alpha");
            if e[3] > 0 {
                assert_eq!(g[..3], e[..3], "pixel {i}: colour");
            }
        }
    }
}

#[test]
fn webp_quality_100_composite_through_a_job_is_exact() {
    let case = Case::fixtures(true, true);
    let dir = TempDir::new("webp_lossless_job");
    let v = view(Mode::Multiply);
    let out = export::run_job(&case.job(dir.path(), v, [false, false, true], Format::Webp, 100));
    assert!(out.error.is_none(), "{:?}", out.error);
    let got = image::open(&out.written[0]).unwrap().to_rgb8().into_raw();
    let expect: Vec<u8> = case.flatten(&v).chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    assert!(got == expect);
}
