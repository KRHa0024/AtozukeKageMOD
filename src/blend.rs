//! Shadow layer over the original with a Photoshop-style blend mode and opacity (canvas and export).

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
    /// PSD blend-mode key.
    pub fn psd_key(self) -> &'static [u8; 4] {
        match self {
            Mode::Multiply => b"mul ",
            Mode::ColorBurn => b"idiv",
            Mode::LinearBurn => b"lbrn",
            Mode::Darken => b"dark",
            Mode::Overlay => b"over",
            Mode::SoftLight => b"sLit",
            Mode::HardLight => b"hLit",
            Mode::Normal => b"norm",
        }
    }

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
    /// RGBA at the original's size, opacity.
    pub ai: Option<(&'a [u8], f32)>,
    /// Layer pixels (1 or 3 channels), channels, mode, opacity.
    pub shadow: Option<(&'a [u8], usize, Mode, f32)>,
}

/// Flatten the visible layers into RGBA (transparent where nothing is visible), like an image editor:
/// the shadow blends with whatever is below it, and shows as itself over transparency.
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
    let blended = composite(&out, layer, ch, mode, op);
    let op = op.clamp(0.0, 1.0);
    for (i, (d, b)) in out.chunks_exact_mut(4).zip(blended.chunks_exact(4)).enumerate() {
        let below = d[3] as f32 / 255.0;
        // Where nothing is below, the layer shows as itself (black with its alpha for Normal).
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
