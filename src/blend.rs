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
}

pub const MODES: [Mode; 7] =
    [Mode::Multiply, Mode::ColorBurn, Mode::LinearBurn, Mode::Darken, Mode::Overlay, Mode::SoftLight, Mode::HardLight];

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
        }
    }

    /// Blend of base `b` and layer `l` (0..1).
    fn apply(self, b: f32, l: f32) -> f32 {
        match self {
            Mode::Multiply => b * l,
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

/// `original` RGBA, `layer` 1 (gray) or 3 bytes per pixel. Returns RGBA (original alpha kept).
pub fn composite(original: &[u8], layer: &[u8], layer_channels: usize, mode: Mode, opacity: f32) -> Vec<u8> {
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
