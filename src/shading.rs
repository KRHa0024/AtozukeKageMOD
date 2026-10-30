//! Shadow layers: the layer that turns an original image into a shaded version of it.
//!
//! Port of tests/reference/shading.py (same math, same resampling; see tests/regression.rs).
//! Given the original O and a shaded version S, the shadow layer L satisfies O x L ~= S. L is solved per pixel at
//! the shaded image's resolution, then brought to the original's resolution by guided upsampling, so its edges
//! follow the full-resolution original. Only darkening is kept (L <= 1).

use crate::resample::{self, Filter};

/// Regularizes the ratio in near-black pixels (ratio of tiny numbers) towards "no shadow".
const RATIO_EPS: f32 = 0.02;
const LUMA: [f32; 3] = [0.299, 0.587, 0.114];

/// An 8-bit image as loaded (alpha kept when the file has one, like a Pillow "RGBA" image).
#[derive(Clone)]
pub struct Image8 {
    pub w: usize,
    pub h: usize,
    pub has_alpha: bool,
    /// RGBA, 4 bytes per pixel (alpha 255 when the file has none).
    pub rgba: Vec<u8>,
}

impl Image8 {
    pub fn from_dynamic(img: &image::DynamicImage) -> Self {
        let has_alpha = img.color().has_alpha();
        let rgba = img.to_rgba8();
        Self { w: rgba.width() as usize, h: rgba.height() as usize, has_alpha, rgba: rgba.into_raw() }
    }

    fn rgb_bytes(&self) -> Vec<u8> {
        self.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
    }

    /// Pillow `image.resize((nw, nh), LANCZOS)` keeping the mode.
    fn resized(&self, nw: usize, nh: usize) -> Image8 {
        let rgba = if self.has_alpha {
            resample::resize_rgba(&self.rgba, self.w, self.h, nw, nh, Filter::Lanczos)
        } else {
            let rgb = resample::resize_u8(&self.rgb_bytes(), self.w, self.h, 3, nw, nh, Filter::Lanczos);
            rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        };
        Image8 { w: nw, h: nh, has_alpha: self.has_alpha, rgba }
    }

    /// Python `_rgb`: transparent images are composited over white, then scaled to 0..1.
    fn to_float(&self) -> Rgb {
        let rgb = if self.has_alpha { resample::over_white(&self.rgba) } else { self.rgb_bytes() };
        Rgb { w: self.w, h: self.h, data: rgb.iter().map(|&v| v as f32 / 255.0).collect() }
    }

    pub fn alpha_is_opaque(&self) -> bool {
        !self.has_alpha || self.rgba.chunks_exact(4).all(|p| p[3] == 255)
    }
}

/// Interleaved float RGB.
struct Rgb {
    w: usize,
    h: usize,
    data: Vec<f32>,
}

impl Rgb {
    fn luma(&self) -> Vec<f32> {
        self.data.chunks_exact(3).map(|p| p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]).collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Per-channel layer (tinted shadows) instead of a grayscale one.
    pub color: bool,
    /// Scales the darkening (0.5 = half as dark, 1.5 = darker).
    pub strength: f32,
    /// Scales the smoothing (0 = raw per-pixel ratio, only resampled). 1.0 = "remove double lines".
    pub smooth: f32,
    /// Treat the brightest areas (97th percentile inside the mask) as unshadowed.
    pub normalize: bool,
    pub radius_frac: f32,
    pub eps: f64,
    pub detail_blur: f32,
    pub tint_limit: Option<f32>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            color: false,
            strength: 1.0,
            smooth: 0.0,
            normalize: true,
            radius_frac: 0.008,
            eps: 5e-3,
            detail_blur: 2.0,
            tint_limit: Some(0.05),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub mean_darkening: f64,
    pub brightened_fraction: f64,
    pub reconstruction_error: f64,
    pub solve_size: (usize, usize),
    pub smooth_radius_px: usize,
}

pub struct Layers {
    pub w: usize,
    pub h: usize,
    /// Multiply layer: 1 byte per pixel (gray) or 3 (color).
    pub multiply: Vec<u8>,
    pub color: bool,
    /// Black RGBA layer whose alpha is the darkening (normal blending).
    pub overlay: Vec<u8>,
    /// original x layer (RGBA; alpha restored from the original when it has transparency).
    pub preview: Vec<u8>,
    pub stats: Stats,
}

/// Mean over a (2r+1)^2 window, edges replicated (Python `_box`: float64 integral image of the padded array).
fn box_mean(a: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let (pw, ph) = (w + 2 * r + 1, h + 2 * r + 1);
    // Padded by r+1 before and r after, like np.pad(mode="edge").
    let mut c = vec![0f64; pw * ph];
    // numpy cumsum(0).cumsum(1) sums columns first; reorder to match its rounding.
    let mut cs = vec![0f64; pw * ph];
    for px in 0..pw {
        let sx = (px as isize - r as isize - 1).clamp(0, w as isize - 1) as usize;
        let mut run = 0.0;
        for py in 0..ph {
            let sy = (py as isize - r as isize - 1).clamp(0, h as isize - 1) as usize;
            run += a[sy * w + sx];
            cs[py * pw + px] = run;
        }
    }
    for py in 0..ph {
        let mut run = 0.0;
        for px in 0..pw {
            run += cs[py * pw + px];
            c[py * pw + px] = run;
        }
    }
    let k = 2 * r + 1;
    let kk = (k * k) as f64;
    let mut out = vec![0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (y1, x1) = (y + k, x + k);
            let s = c[y1 * pw + x1] - c[y * pw + x1] - c[y1 * pw + x] + c[y * pw + x];
            out[y * w + x] = s / kk;
        }
    }
    out
}

/// Smoothed linear coefficients (A, B) of the guided filter: output = A * guide + B.
fn guided_coefficients(guide: &[f32], src: &[f32], w: usize, h: usize, r: usize, eps: f64) -> (Vec<f32>, Vec<f32>) {
    let g: Vec<f64> = guide.iter().map(|&v| v as f64).collect();
    let p: Vec<f64> = src.iter().map(|&v| v as f64).collect();
    let gp: Vec<f64> = guide.iter().zip(src).map(|(&a, &b)| (a * b) as f64).collect();
    let gg: Vec<f64> = guide.iter().map(|&a| (a * a) as f64).collect();
    let mean_i = box_mean(&g, w, h, r);
    let mean_p = box_mean(&p, w, h, r);
    let corr_ip = box_mean(&gp, w, h, r);
    let corr_ii = box_mean(&gg, w, h, r);
    let mut a = vec![0f64; w * h];
    let mut b = vec![0f64; w * h];
    for i in 0..w * h {
        let cov = corr_ip[i] - mean_i[i] * mean_p[i];
        let var = corr_ii[i] - mean_i[i] * mean_i[i];
        a[i] = cov / (var + eps);
        b[i] = mean_p[i] - a[i] * mean_i[i];
    }
    let a = box_mean(&a, w, h, r).into_iter().map(|v| v as f32).collect();
    let b = box_mean(&b, w, h, r).into_iter().map(|v| v as f32).collect();
    (a, b)
}

/// numpy `percentile(values, q)` with linear interpolation.
fn percentile(mut values: Vec<f32>, q: f64) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    let pos = q / 100.0 * (values.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(values.len() - 1);
    let (_, v_lo, rest) = values.select_nth_unstable_by(lo, |a, b| a.total_cmp(b));
    let v_lo = *v_lo as f64;
    let v_hi = if hi == lo { v_lo } else { rest.iter().copied().fold(f32::INFINITY, f32::min) as f64 };
    v_lo + (v_hi - v_lo) * (pos - lo as f64)
}

/// Python's round(): halves go to the even neighbour.
fn py_round(v: f64) -> f64 {
    v.round_ties_even()
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Solve the shadow layer for `original` -> `shaded`. `mask`: white = shadows allowed (full resolution of the
/// mask image, any size; resized like the Python code).
pub fn shadow_layers(original: &Image8, shaded: &Image8, mask: Option<(&[u8], usize, usize)>, opt: &Options) -> Layers {
    let o = original.to_float();
    let (w, h) = (o.w, o.h);
    let (mut lw, mut lh) = (shaded.w, shaded.h);
    if lw * lh >= w * h {
        (lw, lh) = (w, h);
    }
    let s_lo = if (shaded.w, shaded.h) != (lw, lh) { shaded.resized(lw, lh).to_float() } else { shaded.to_float() };
    let o_lo_img;
    let o_lo = if (lw, lh) != (w, h) {
        o_lo_img = original.resized(lw, lh).to_float();
        &o_lo_img
    } else {
        &o
    };
    let guide_lo = o_lo.luma();
    let guide = o.luma();
    let m: Option<Vec<f32>> = mask.map(|(px, mw, mh)| {
        resample::resize_u8(px, mw, mh, 1, w, h, Filter::Lanczos).iter().map(|&v| v as f32 / 255.0).collect()
    });
    let m_lo = m.as_ref().map(|m| resample::resize_f32(m, w, h, lw, lh, Filter::Bilinear));

    let ratio_of = |o: &[f32], s: &[f32]| -> Vec<f32> {
        if opt.color {
            o.iter().zip(s).map(|(&o, &s)| (o * s + RATIO_EPS / 3.0) / (o * o + RATIO_EPS / 3.0)).collect()
        } else {
            o.chunks_exact(3)
                .zip(s.chunks_exact(3))
                .map(|(o, s)| {
                    let os = o[0] * s[0] + o[1] * s[1] + o[2] * s[2];
                    let oo = o[0] * o[0] + o[1] * o[1] + o[2] * o[2];
                    (os + RATIO_EPS) / (oo + RATIO_EPS)
                })
                .collect()
        }
    };
    let mut ratio = if opt.detail_blur > 0.0 && opt.smooth > 0.0 {
        // Ignore fine re-drawn details (hair strands) of the model output: 3 box passes ~ Gaussian.
        let sigma = opt.smooth as f64 * opt.detail_blur as f64 * lw.max(lh) as f64 / 1000.0;
        let r = (py_round(sigma) as usize).max(1);
        let blur = |img: &Rgb| -> Vec<f32> {
            let mut out = vec![0f32; img.data.len()];
            for c in 0..3 {
                let mut plane: Vec<f64> = img.data.iter().skip(c).step_by(3).map(|&v| v as f64).collect();
                for _ in 0..3 {
                    plane = box_mean(&plane, lw, lh, r);
                }
                for (i, v) in plane.into_iter().enumerate() {
                    out[i * 3 + c] = v as f32;
                }
            }
            out
        };
        ratio_of(&blur(o_lo), &blur(&s_lo))
    } else {
        ratio_of(&o_lo.data, &s_lo.data)
    };
    if opt.color {
        if let Some(limit) = opt.tint_limit {
            for p in ratio.chunks_exact_mut(3) {
                let g = p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2];
                for v in p.iter_mut() {
                    *v = g + (*v - g).clamp(-limit, limit);
                }
            }
        }
    }
    let brightened = ratio.iter().filter(|&&v| v > 1.02).count() as f64 / ratio.len() as f64;
    let mut channels: Vec<Vec<f32>> = if opt.color {
        (0..3).map(|c| ratio.iter().skip(c).step_by(3).copied().collect()).collect()
    } else {
        vec![ratio.clone()]
    };
    if opt.normalize {
        let lum: Vec<f32> = if opt.color {
            ratio.chunks_exact(3).map(|p| p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]).collect()
        } else {
            ratio.clone()
        };
        let picked: Vec<f32> = match &m_lo {
            Some(m) => lum.iter().zip(m).filter(|(_, &m)| m > 0.5).map(|(&v, _)| v).collect(),
            None => lum,
        };
        let lit = if picked.is_empty() { 1.0 } else { percentile(picked, 97.0) };
        if lit > 0.05 {
            for c in &mut channels {
                for v in c.iter_mut() {
                    *v /= lit as f32; // numpy 2: float32 array / Python float stays float32
                }
            }
        }
    }
    let radius_lo = (py_round(opt.smooth.max(0.0) as f64 * opt.radius_frac as f64 * lw.max(lh) as f64) as usize).max(1);
    let upscale = (w, h) != (lw, lh);
    let solve = |c: Vec<f32>| -> Vec<f32> {
        if opt.smooth <= 0.0 && !upscale {
            return c;
        }
        let (mut a, mut b) = guided_coefficients(&guide_lo, &c, lw, lh, radius_lo, opt.eps);
        if upscale {
            a = resample::resize_f32(&a, lw, lh, w, h, Filter::Bilinear);
            b = resample::resize_f32(&b, lw, lh, w, h, Filter::Bilinear);
        }
        a.iter().zip(&b).zip(&guide).map(|((&a, &b), &g)| a * g + b).collect()
    };
    let layer_channels: Vec<Vec<f32>> = std::thread::scope(|s| {
        let handles: Vec<_> = channels.into_iter().map(|c| s.spawn(|| solve(c))).collect();
        handles.into_iter().map(|h| h.join().expect("shadow layer worker")).collect()
    });

    let strength = opt.strength.max(0.0);
    let nch = layer_channels.len();
    let mut layer = vec![0f32; w * h * nch];
    for (c, ch) in layer_channels.iter().enumerate() {
        for (i, &v) in ch.iter().enumerate() {
            let mut v = 1.0 - (1.0 - v.clamp(0.0, 1.0)) * strength;
            if let Some(m) = &m {
                v = 1.0 - (1.0 - v) * m[i];
            }
            layer[i * nch + c] = v.clamp(0.0, 1.0);
        }
    }
    let gray: Vec<f32> = if nch == 3 {
        layer.chunks_exact(3).map(|p| p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]).collect()
    } else {
        layer.clone()
    };
    let mut preview_rgb = vec![0u8; w * h * 3];
    for i in 0..w * h {
        for c in 0..3 {
            let l = if nch == 3 { layer[i * 3 + c] } else { layer[i] };
            preview_rgb[i * 3 + c] = to8(o.data[i * 3 + c] * l);
        }
    }
    let mut overlay = vec![0u8; w * h * 4];
    for (i, &g) in gray.iter().enumerate() {
        overlay[i * 4 + 3] = to8(1.0 - g);
    }
    let preview_lo = resample::resize_u8(&preview_rgb, w, h, 3, lw, lh, Filter::Lanczos);
    let error = preview_lo.iter().zip(&s_lo.data).map(|(&p, &s)| (p as f32 / 255.0 - s).abs() as f64).sum::<f64>()
        / preview_lo.len() as f64
        * 255.0;
    let mean_gray = gray.iter().map(|&v| v as f64).sum::<f64>() / gray.len() as f64;

    // comp.restore_alpha: re-attach the source's transparency.
    let keep_alpha = !original.alpha_is_opaque();
    let preview = preview_rgb
        .chunks_exact(3)
        .enumerate()
        .flat_map(|(i, p)| [p[0], p[1], p[2], if keep_alpha { original.rgba[i * 4 + 3] } else { 255 }])
        .collect();
    Layers {
        w,
        h,
        multiply: layer.iter().map(|&v| to8(v)).collect(),
        color: nch == 3,
        overlay,
        preview,
        stats: Stats {
            mean_darkening: 1.0 - mean_gray,
            brightened_fraction: brightened,
            reconstruction_error: error,
            solve_size: (lw, lh),
            smooth_radius_px: radius_lo,
        },
    }
}

/// Subject mask from a background-removal cutout (alpha channel), softened with Pillow's
/// `GaussianBlur(1.5)` as in the reference pipeline.
pub fn subject_mask(cutout: &image::DynamicImage) -> (Vec<u8>, usize, usize) {
    let rgba = cutout.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let alpha: Vec<u8> = rgba.pixels().map(|p| p[3]).collect();
    (resample::gaussian_blur_l(&alpha, w, h, 1.5), w, h)
}
