//! Shadow layer over the original with a Photoshop-style blend mode and opacity (canvas and export).

use crate::shading::{Image8, Layers};

/// Layers of the layer panel, top to bottom (indexes of `View::visible` and `View::opacity`).
pub const SHADOW: usize = 0;
pub const AI: usize = 1;
pub const ORIGINAL: usize = 2;

/// What the layer panel shows; canvas and exports both flatten it through `flatten_view`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    pub visible: [bool; 3],
    /// The original's stays 1.
    pub opacity: [f32; 3],
    pub mode: Mode,
    /// Where the shadow layer was moved to, in image pixels.
    pub shadow_offset: (i32, i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Multiply,
    ColorBurn,
    LinearBurn,
    Darken,
    Overlay,
    SoftLight,
    HardLight,
    /// The black layer with alpha = darkening (the gray of the layer), blended normally.
    Normal,
}

/// Indexed by the saved setting (Normal was added last, so older settings keep their meaning).
pub const MODES: [Mode; 8] = [
    Mode::Multiply,
    Mode::ColorBurn,
    Mode::LinearBurn,
    Mode::Darken,
    Mode::Overlay,
    Mode::SoftLight,
    Mode::HardLight,
    Mode::Normal,
];
/// Order in the blend-mode menu (Normal first, as in image editors).
pub const MENU_ORDER: [usize; 8] = [7, 0, 1, 2, 3, 4, 5, 6];

impl Mode {
    /// Blend of base `b` and layer `l` (0..1).
    fn apply(self, b: f32, l: f32) -> f32 {
        match self {
            // Normal: black at alpha (1 - gray) over the base is the base times the gray layer.
            Mode::Multiply | Mode::Normal => b * l,
            Mode::ColorBurn => {
                if b >= 1.0 {
                    1.0
                } else if l <= 0.0 {
                    0.0
                } else {
                    1.0 - ((1.0 - b) / l).min(1.0)
                }
            }
            Mode::LinearBurn => (b + l - 1.0).max(0.0),
            Mode::Darken => b.min(l),
            Mode::Overlay => Mode::HardLight.apply(l, b),
            Mode::HardLight => {
                if l <= 0.5 {
                    2.0 * b * l
                } else {
                    1.0 - 2.0 * (1.0 - b) * (1.0 - l)
                }
            }
            Mode::SoftLight => {
                if l <= 0.5 {
                    b - (1.0 - 2.0 * l) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                    b + (2.0 * l - 1.0) * (d - b)
                }
            }
        }
    }
}

/// 256x256 lookup per mode (base, layer) -> blended value.
fn table(mode: Mode) -> Vec<u8> {
    let mut t = vec![0u8; 256 * 256];
    for b in 0..256 {
        for l in 0..256 {
            let v = mode.apply(b as f32 / 255.0, l as f32 / 255.0);
            t[b * 256 + l] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    t
}

/// Gray of an RGB layer (the alpha of the normal-blend layer is 255 - this).
fn gray_of(layer: &[u8], layer_channels: usize, i: usize) -> u8 {
    if layer_channels == 1 {
        return layer[i];
    }
    let p = &layer[i * 3..i * 3 + 3];
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32 + 0.5) as u8
}

/// `original` RGBA, `layer` 1 (gray) or 3 bytes per pixel. Returns RGBA (original alpha kept).
pub fn composite(original: &[u8], layer: &[u8], layer_channels: usize, mode: Mode, opacity: f32) -> Vec<u8> {
    if mode == Mode::Normal && layer_channels == 3 {
        let gray: Vec<u8> = (0..original.len() / 4).map(|i| gray_of(layer, 3, i)).collect();
        return composite(original, &gray, 1, mode, opacity);
    }
    let n = original.len() / 4;
    let lut = table(mode);
    let a = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    let mut out = vec![0u8; n * 4];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (ci, dst) in out.chunks_mut(chunk * 4).enumerate() {
            let lut = &lut;
            s.spawn(move || {
                let start = ci * chunk;
                for (k, px) in dst.chunks_exact_mut(4).enumerate() {
                    let i = start + k;
                    for c in 0..3 {
                        let b = original[i * 4 + c] as u32;
                        let l = layer[i * layer_channels + if layer_channels == 3 { c } else { 0 }] as usize;
                        let v = lut[b as usize * 256 + l] as u32;
                        px[c] = ((b * (256 - a) + v * a + 128) >> 8) as u8;
                    }
                    px[3] = original[i * 4 + 3];
                }
            });
        }
    });
    out
}

/// The visible layers of the layer panel, bottom to top: original, AI output (normal blend), shadow.
pub struct Stack<'a> {
    pub original: Option<&'a [u8]>,
    /// The source image's RGBA, visible or not: the shadow never leaves its silhouette.
    pub silhouette: &'a [u8],
    /// RGBA at the original's size, opacity.
    pub ai: Option<(&'a [u8], f32)>,
    /// Layer pixels (1 or 3 channels), channels, mode, opacity.
    pub shadow: Option<(&'a [u8], usize, Mode, f32)>,
    /// Where the shadow layer was moved to, in image pixels; what it leaves uncovered shows no shadow at all.
    pub shadow_offset: (i32, i32),
    /// Image width (the height follows from the pixel count).
    pub width: usize,
}

/// `pixels` (`bytes` per pixel) moved by `offset`; what is uncovered is `fill`.
fn moved(pixels: &[u8], bytes: usize, w: usize, (dx, dy): (i32, i32), fill: u8) -> Vec<u8> {
    let h = pixels.len() / bytes / w;
    let mut out = vec![fill; pixels.len()];
    let shift = dx.unsigned_abs() as usize;
    if shift >= w {
        return out;
    }
    let row_bytes = (w - shift) * bytes;
    for y in 0..h {
        let target = y as i64 + dy as i64;
        if target < 0 || target >= h as i64 {
            continue;
        }
        let (from, to) = if dx >= 0 { (0, shift) } else { (shift, 0) };
        let src = y * w * bytes + from * bytes;
        let dst = target as usize * w * bytes + to * bytes;
        out[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
    }
    out
}

/// Flatten the visible layers into RGBA (transparent where nothing is visible), like an image editor: the shadow
/// blends with whatever is below it. It adds no coverage of its own over the original, so a transparent source stays
/// transparent; with the original hidden it shows as itself, inside the source's silhouette.
pub fn flatten(stack: &Stack, pixels: usize) -> Vec<u8> {
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
    let (w, (dx, dy)) = (stack.width, stack.shadow_offset);
    let shifted;
    let layer = if (dx, dy) == (0, 0) {
        layer
    } else {
        shifted = moved(layer, ch, w, (dx, dy), 255);
        shifted.as_slice()
    };
    let h = (pixels / w) as i32;
    let covered = |i: usize| {
        let (x, y) = ((i % w) as i32, (i / w) as i32);
        x >= dx && x < w as i32 + dx && y >= dy && y < h + dy
    };
    let blended = composite(&out, layer, ch, mode, op);
    let op = op.clamp(0.0, 1.0);
    let over_original = stack.original.is_some();
    let over_ai = stack.ai.is_some();
    for (i, (d, b)) in out.chunks_exact_mut(4).zip(blended.chunks_exact(4)).enumerate() {
        if !covered(i) {
            continue;
        }
        let source_alpha = stack.silhouette[i * 4 + 3];
        let keep = source_alpha as f32 / 255.0;
        let below = d[3] as f32 / 255.0;
        // Where nothing is below, the layer shows as itself (black with its alpha for Normal).
        let (rgb, la) = if mode == Mode::Normal {
            ([0u8; 3], (255 - gray_of(layer, ch, i)) as f32 / 255.0 * op)
        } else if ch == 3 {
            ([layer[i * 3], layer[i * 3 + 1], layer[i * 3 + 2]], op)
        } else {
            ([layer[i]; 3], op)
        };
        let mut shaded = [b[0], b[1], b[2]];
        if source_alpha != 255 && over_ai {
            // Over the AI layer the shadow fades out with the silhouette (like a layer with the source's alpha).
            for c in 0..3 {
                shaded[c] = (d[c] as f32 + (b[c] as f32 - d[c] as f32) * keep + 0.5) as u8;
            }
        }
        let own = if over_original { 0.0 } else { la * keep * (1.0 - below) };
        let oa = below + own;
        if oa > 0.0 {
            for c in 0..3 {
                d[c] = ((shaded[c] as f32 * below + rgb[c] as f32 * own) / oa + 0.5) as u8;
            }
        }
        d[3] = (oa * 255.0 + 0.5) as u8;
    }
    out
}

/// The layer panel flattened: what the canvas shows and the composite export writes.
pub fn flatten_view(original: &Image8, ai_full: &[u8], layers: &Layers, view: &View) -> Vec<u8> {
    let channels = if layers.color { 3 } else { 1 };
    flatten(
        &Stack {
            original: view.visible[ORIGINAL].then_some(original.rgba.as_slice()),
            silhouette: &original.rgba,
            ai: view.visible[AI].then_some((ai_full, view.opacity[AI])),
            shadow: view.visible[SHADOW].then_some((layers.multiply.as_slice(), channels, view.mode, view.opacity[SHADOW])),
            shadow_offset: view.shadow_offset,
            width: layers.w,
        },
        layers.w * layers.h,
    )
}

/// The shadow as a layer of its own (RGBA, inside the source's silhouette): the multiply layer, or for Normal the
/// black layer with alpha.
pub fn shadow_rgba(layers: &Layers, mode: Mode, original: &[u8], offset: (i32, i32)) -> Vec<u8> {
    let rgba: Vec<u8> = if mode == Mode::Normal {
        layers.overlay.chunks_exact(4).flat_map(|p| [0, 0, 0, p[3]]).collect()
    } else if layers.color {
        layers.multiply.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
    } else {
        layers.multiply.iter().flat_map(|&v| [v, v, v, 255]).collect()
    };
    let mut rgba = moved(&rgba, 4, layers.w, offset, 0);
    for (p, source) in rgba.chunks_exact_mut(4).zip(original.chunks_exact(4)) {
        p[3] = scale_alpha(p[3], source[3]);
    }
    rgba
}

fn scale_alpha(alpha: u8, by: u8) -> u8 {
    ((alpha as u32 * by as u32 + 127) / 255) as u8
}
