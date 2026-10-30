//! Layer-stack flattening (canvas and composite export): equality with the behaviour before the transparency fix for
//! opaque sources, and the transparency rules for sources with alpha.

use std::path::PathBuf;

use kage::blend::{composite, flatten, flatten_view, shadow_rgba, Mode, Stack, View, AI, DEFAULT_MODE, MODES, ORIGINAL, SHADOW};
use kage::export::{self, Pixels};
use kage::shading::{self, Image8, Layers, Options, Stats};

#[test]
fn the_default_mode_is_linear_burn() {
    assert_eq!(MODES[DEFAULT_MODE], Mode::LinearBurn);
}

fn fixture(name: &str) -> image::DynamicImage {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    image::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Deterministic pseudo-random bytes (xorshift), with the extremes 0 and 255 mixed in.
fn noise(len: usize, mut seed: u32) -> Vec<u8> {
    (0..len)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            match i % 11 {
                0 => 0,
                1 => 255,
                _ => (seed >> 8) as u8,
            }
        })
        .collect()
}

/// `flatten` as it was before the transparency fix: no silhouette, and the shadow's own coverage is
/// `la * (1 - below)` whether or not the original is visible.
fn legacy_flatten(stack: &Stack, pixels: usize) -> Vec<u8> {
    fn gray_of(layer: &[u8], layer_channels: usize, i: usize) -> u8 {
        if layer_channels == 1 {
            return layer[i];
        }
        let p = &layer[i * 3..i * 3 + 3];
        (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32 + 0.5) as u8
    }
    let mut out = match stack.original {
        Some(o) => o.to_vec(),
        None => vec![0u8; pixels * 4],
    };
    if let Some((ai, op)) = stack.ai {
        for (d, s) in out.chunks_exact_mut(4).zip(ai.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0 * op.clamp(0.0, 1.0);
            let da = d[3] as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            if oa > 0.0 {
                for c in 0..3 {
                    d[c] = ((s[c] as f32 * a + d[c] as f32 * da * (1.0 - a)) / oa + 0.5) as u8;
                }
            }
            d[3] = (oa * 255.0 + 0.5) as u8;
        }
    }
    let Some((layer, ch, mode, op)) = stack.shadow else { return out };
    let blended = composite(&out, layer, ch, mode, op);
    let op = op.clamp(0.0, 1.0);
    for (i, (d, b)) in out.chunks_exact_mut(4).zip(blended.chunks_exact(4)).enumerate() {
        let below = d[3] as f32 / 255.0;
        let (rgb, la) = if mode == Mode::Normal {
            ([0u8; 3], (255 - gray_of(layer, ch, i)) as f32 / 255.0 * op)
        } else if ch == 3 {
            ([layer[i * 3], layer[i * 3 + 1], layer[i * 3 + 2]], op)
        } else {
            ([layer[i]; 3], op)
        };
        let own = la * (1.0 - below);
        let oa = below + own;
        if oa > 0.0 {
            for c in 0..3 {
                d[c] = ((b[c] as f32 * below + rgb[c] as f32 * own) / oa + 0.5) as u8;
            }
        }
        d[3] = (oa * 255.0 + 0.5) as u8;
    }
    out
}

fn synthetic_layers(w: usize, h: usize, color: bool, seed: u32) -> Layers {
    let n = w * h;
    let multiply = noise(n * if color { 3 } else { 1 }, seed);
    let mut overlay = vec![0u8; n * 4];
    for (i, p) in overlay.chunks_exact_mut(4).enumerate() {
        p[3] = 255 - multiply[if color { i * 3 } else { i }];
    }
    Layers { w, h, multiply, color, overlay, preview: vec![0; n * 4], stats: Stats::default() }
}

#[test]
fn opaque_source_is_byte_identical_to_legacy_flatten() {
    let (w, h) = (17, 13);
    let n = w * h;
    let mut rgba = noise(n * 4, 0x1234_5678);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    let original = Image8 { w, h, has_alpha: false, rgba: rgba.clone(), icc: None, deep: false };
    // The AI output has varying alpha too: the opaque-source guarantee must not depend on it.
    let ai = noise(n * 4, 0x9e37_79b9);
    let mut compared = 0usize;
    for (color, seed) in [(false, 11u32), (true, 22)] {
        let layers = synthetic_layers(w, h, color, seed);
        let ch = if color { 3 } else { 1 };
        for mode in MODES {
            for shadow_opacity in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                for ai_opacity in [0.5f32, 1.0] {
                    for bits in 0..8u8 {
                        let visible = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
                        let view = View { visible, opacity: [shadow_opacity, ai_opacity, 1.0], mode, shadow_offset: (0, 0) };
                        let stack = Stack {
                            original: visible[ORIGINAL].then_some(rgba.as_slice()),
                            silhouette: &rgba,
                            ai: visible[AI].then_some((ai.as_slice(), ai_opacity)),
                            shadow: visible[SHADOW].then_some((layers.multiply.as_slice(), ch, mode, shadow_opacity)),
                            shadow_offset: (0, 0),
                            width: w,
                        };
                        let expect = legacy_flatten(&stack, n);
                        let ours = flatten(&stack, n);
                        let via_view = flatten_view(&original, &ai, &layers, &view);
                        let what = format!("color={color} {mode:?} shadow {shadow_opacity} ai {ai_opacity} visible {visible:?}");
                        if ours != expect {
                            let i = ours.iter().zip(&expect).position(|(a, b)| a != b).unwrap();
                            panic!("{what}: flatten differs from legacy at byte {i}: {} vs {}", ours[i], expect[i]);
                        }
                        assert!(via_view == expect, "{what}: flatten_view differs from legacy");
                        compared += 1;
                    }
                }
            }
        }
    }
    assert_eq!(compared, 2 * 8 * 5 * 2 * 8);
}

/// The fixture original (transparent, semi-transparent and opaque pixels), its AI output and a solved colour layer.
struct Fixture {
    original: Image8,
    ai_full: Vec<u8>,
    layers: Layers,
}

fn fixture_with(color: bool) -> Fixture {
    let original = Image8::from_dynamic(&fixture("original.png"));
    let shaded = Image8::from_dynamic(&fixture("shaded.png"));
    let layers = shading::shadow_layers(&original, &shaded, None, &Options { color, ..Options::default() });
    let ai_full = export::to_canvas_rgba(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, layers.w, layers.h);
    Fixture { original, ai_full, layers }
}

#[test]
fn fixture_has_the_three_kinds_of_alpha() {
    let f = fixture_with(false);
    let alphas: std::collections::BTreeSet<u8> = f.original.rgba.chunks_exact(4).map(|p| p[3]).collect();
    assert!(alphas.contains(&0) && alphas.contains(&255) && alphas.iter().any(|&a| a != 0 && a != 255), "{alphas:?}");
}

#[test]
fn original_alpha_is_kept_for_every_mode() {
    for color in [false, true] {
        let f = fixture_with(color);
        let alpha: Vec<u8> = f.original.rgba.chunks_exact(4).map(|p| p[3]).collect();
        for mode in MODES {
            for ai_visible in [false, true] {
                for shadow_opacity in [0.3f32, 1.0] {
                    let view = View { visible: [true, ai_visible, true], opacity: [shadow_opacity, 0.7, 1.0], mode, shadow_offset: (0, 0) };
                    let out = flatten_view(&f.original, &f.ai_full, &f.layers, &view);
                    let got: Vec<u8> = out.chunks_exact(4).map(|p| p[3]).collect();
                    // With the AI layer visible its own alpha (opaque here) is allowed to cover; only the plain
                    // original + shadow case must keep the original's alpha exactly.
                    if !ai_visible {
                        assert!(got == alpha, "color={color} {mode:?} opacity {shadow_opacity}: alpha changed");
                    } else {
                        // Over a transparent source the opaque AI layer is its own coverage; it is never less
                        // than the source's.
                        assert!(got.iter().zip(&alpha).all(|(g, a)| g >= a), "color={color} {mode:?}: alpha shrank");
                    }
                }
            }
        }
    }
}

#[test]
fn shadow_alone_stays_inside_the_silhouette() {
    for color in [false, true] {
        let f = fixture_with(color);
        let alpha: Vec<u8> = f.original.rgba.chunks_exact(4).map(|p| p[3]).collect();
        for mode in MODES {
            let view = View { visible: [true, false, false], opacity: [1.0, 1.0, 1.0], mode, shadow_offset: (0, 0) };
            let out = flatten_view(&f.original, &f.ai_full, &f.layers, &view);
            let mut any = false;
            for (i, (p, &a)) in out.chunks_exact(4).zip(&alpha).enumerate() {
                if a == 0 {
                    assert_eq!(p, [0, 0, 0, 0], "color={color} {mode:?}: pixel {i} appears outside the silhouette");
                }
                assert!(p[3] <= a, "color={color} {mode:?}: pixel {i} alpha {} exceeds the source alpha {a}", p[3]);
                any |= p[3] > 0;
            }
            assert!(any, "color={color} {mode:?}: nothing is shown at all");
        }
    }
}

#[test]
fn shadow_alone_over_opaque_pixels_shows_itself() {
    // Where the source is fully opaque the shadow alone has the layer's coverage (opacity 1 for blend modes).
    let f = fixture_with(true);
    let view = View { visible: [true, false, false], opacity: [1.0, 1.0, 1.0], mode: Mode::Multiply, shadow_offset: (0, 0) };
    let out = flatten_view(&f.original, &f.ai_full, &f.layers, &view);
    for (i, p) in out.chunks_exact(4).enumerate() {
        if f.original.rgba[i * 4 + 3] == 255 {
            assert_eq!(p, [f.layers.multiply[i * 3], f.layers.multiply[i * 3 + 1], f.layers.multiply[i * 3 + 2], 255], "pixel {i}");
        }
    }
}

fn scale_alpha(alpha: u8, by: u8) -> u8 {
    ((alpha as u32 * by as u32 + 127) / 255) as u8
}

#[test]
fn shadow_rgba_multiply_keeps_layer_colour_with_source_alpha() {
    for color in [false, true] {
        let f = fixture_with(color);
        let out = shadow_rgba(&f.layers, Mode::Multiply, &f.original.rgba, (0, 0));
        assert_eq!(out.len(), f.layers.w * f.layers.h * 4);
        for (i, p) in out.chunks_exact(4).enumerate() {
            let a = f.original.rgba[i * 4 + 3];
            let rgb = if color {
                [f.layers.multiply[i * 3], f.layers.multiply[i * 3 + 1], f.layers.multiply[i * 3 + 2]]
            } else {
                [f.layers.multiply[i]; 3]
            };
            assert_eq!(p, [rgb[0], rgb[1], rgb[2], a], "color={color} pixel {i}");
        }
    }
}

#[test]
fn shadow_rgba_normal_is_black_with_scaled_alpha() {
    for color in [false, true] {
        let f = fixture_with(color);
        let out = shadow_rgba(&f.layers, Mode::Normal, &f.original.rgba, (0, 0));
        assert_eq!(out.len(), f.layers.w * f.layers.h * 4);
        for (i, p) in out.chunks_exact(4).enumerate() {
            let expect = scale_alpha(f.layers.overlay[i * 4 + 3], f.original.rgba[i * 4 + 3]);
            assert_eq!(p, [0, 0, 0, expect], "color={color} pixel {i}");
        }
        // The fixture really has darkening, so the check above is not vacuous.
        assert!(out.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));

        // An opaque source gives the overlay itself.
        let opaque: Vec<u8> = f.original.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let out = shadow_rgba(&f.layers, Mode::Normal, &opaque, (0, 0));
        assert!(out == f.layers.overlay, "color={color}: opaque source must equal layers.overlay");
    }
}

/// One dark layer pixel at (2, 1) over a flat opaque image.
fn dot(w: usize, h: usize) -> (Image8, Layers) {
    let rgba: Vec<u8> = (0..w * h).flat_map(|_| [200, 150, 100, 255]).collect();
    let mut layers = synthetic_layers(w, h, false, 1);
    layers.multiply = vec![255; w * h];
    layers.multiply[w + 2] = 0;
    layers.overlay = layers.multiply.iter().flat_map(|&v| [0, 0, 0, 255 - v]).collect();
    (Image8 { w, h, has_alpha: false, rgba, icc: None, deep: false }, layers)
}

#[test]
fn moved_shadow_lands_at_the_offset() {
    let (w, h) = (8, 6);
    let (original, layers) = dot(w, h);
    let view = View { visible: [true, false, true], opacity: [1.0; 3], mode: Mode::Multiply, shadow_offset: (3, 2) };
    let out = flatten_view(&original, &[], &layers, &view);
    for y in 0..h {
        for x in 0..w {
            let px = &out[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let expect: [u8; 4] = if (x, y) == (5, 3) { [0, 0, 0, 255] } else { [200, 150, 100, 255] };
            assert_eq!(px, expect, "({x}, {y})");
        }
    }
}

#[test]
fn what_a_moved_shadow_uncovers_is_left_alone_in_every_mode() {
    // White is not neutral in Overlay and the like, so the uncovered part must skip the blend altogether.
    let (w, h) = (8, 6);
    let (original, layers) = dot(w, h);
    for mode in MODES {
        let view = View { visible: [true, false, true], opacity: [1.0; 3], mode, shadow_offset: (2, 1) };
        let out = flatten_view(&original, &[], &layers, &view);
        for y in 0..h {
            for x in 0..w {
                if x < 2 || y < 1 {
                    assert_eq!(&out[(y * w + x) * 4..(y * w + x) * 4 + 4], [200, 150, 100, 255], "{mode:?} ({x}, {y})");
                }
            }
        }
    }
}

#[test]
fn a_shadow_moved_off_the_image_leaves_it_unchanged() {
    let (w, h) = (8, 6);
    let (original, layers) = dot(w, h);
    let view = View { visible: [true, false, true], opacity: [1.0; 3], mode: Mode::Overlay, shadow_offset: (w as i32, 0) };
    assert!(flatten_view(&original, &[], &layers, &view) == original.rgba);
}

#[test]
fn shadow_layer_file_moves_too() {
    let (w, h) = (8, 6);
    let (original, layers) = dot(w, h);
    let out = shadow_rgba(&layers, Mode::Multiply, &original.rgba, (-1, 1));
    assert_eq!(&out[(2 * w + 1) * 4..(2 * w + 1) * 4 + 4], [0, 0, 0, 255], "the dot moved from (2, 1) to (1, 2)");
    assert_eq!(out[(w + 2) * 4 + 3], 255);
    assert_eq!(out[3], 0, "the top row is uncovered");
    assert_eq!(out[(2 * w - 2) * 4 + 3], 255, "(w - 2, 1) is covered");
    assert_eq!(out[(2 * w - 1) * 4 + 3], 0, "the right column is uncovered too");
}
