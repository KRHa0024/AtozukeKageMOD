//! Pillow-compatible resampling and blurring, so the shadow layer matches the Python reference
//! (tests/reference/shading.py) value for value instead of "about the same".
//!
//! Ported from Pillow's libImaging (Resample.c, BoxBlur.c, AlphaComposite.c, Convert.c).

use std::f64::consts::PI;

#[derive(Clone, Copy)]
pub enum Filter {
    Bilinear,
    Lanczos,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Lanczos => 3.0,
        }
    }

    fn eval(self, x: f64) -> f64 {
        match self {
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Filter::Lanczos => {
                if (-3.0..3.0).contains(&x) {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * PI;
    x.sin() / x
}

/// Per output pixel: first input index and normalized weights (Pillow `precompute_coeffs`).
struct Coeffs {
    ksize: usize,
    bounds: Vec<(usize, usize)>,
    kk: Vec<f64>,
}

fn precompute(in_size: usize, out_size: usize, filter: Filter) -> Coeffs {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0.0; out_size * ksize];
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        // C casts truncate toward zero; negative values are clamped to 0 right after.
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
        let n = xmax.saturating_sub(xmin);
        let k = &mut kk[xx * ksize..xx * ksize + ksize];
        let mut ww = 0.0;
        for x in 0..n {
            let w = filter.eval((x as f64 + xmin as f64 - center + 0.5) * ss);
            k[x] = w;
            ww += w;
        }
        if ww != 0.0 {
            for v in k.iter_mut().take(n) {
                *v /= ww;
            }
        }
        bounds.push((xmin, n));
    }
    Coeffs { ksize, bounds, kk }
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn fixed_coeffs(c: &Coeffs) -> Vec<i32> {
    c.kk
        .iter()
        .map(|&v| {
            let s = v * (1u32 << PRECISION_BITS) as f64;
            if v < 0.0 {
                (-0.5 + s) as i32
            } else {
                (0.5 + s) as i32
            }
        })
        .collect()
}

fn clip8(v: i64) -> u8 {
    if v >= (1i64 << PRECISION_BITS << 8) {
        255
    } else if v <= 0 {
        0
    } else {
        (v >> PRECISION_BITS) as u8
    }
}

/// Resize an interleaved 8-bit image (`ch` channels) like Pillow: horizontal pass, then vertical, with 8-bit
/// rounding in between.
pub fn resize_u8(src: &[u8], w: usize, h: usize, ch: usize, nw: usize, nh: usize, filter: Filter) -> Vec<u8> {
    if (w, h) == (nw, nh) {
        return src.to_vec();
    }
    let mut cur = src.to_vec();
    let mut cw = w;
    if nw != w {
        let c = precompute(w, nw, filter);
        let k = fixed_coeffs(&c);
        let mut out = vec![0u8; nw * h * ch];
        for y in 0..h {
            let row = &cur[y * w * ch..(y + 1) * w * ch];
            let orow = &mut out[y * nw * ch..(y + 1) * nw * ch];
            for (xx, &(xmin, n)) in c.bounds.iter().enumerate() {
                let kx = &k[xx * c.ksize..xx * c.ksize + n];
                for b in 0..ch {
                    let mut ss: i64 = 1 << (PRECISION_BITS - 1);
                    for (x, &kv) in kx.iter().enumerate() {
                        ss += row[(xmin + x) * ch + b] as i64 * kv as i64;
                    }
                    orow[xx * ch + b] = clip8(ss);
                }
            }
        }
        cur = out;
        cw = nw;
    }
    if nh != h {
        let c = precompute(h, nh, filter);
        let k = fixed_coeffs(&c);
        let stride = cw * ch;
        let mut out = vec![0u8; nh * stride];
        for (yy, &(ymin, n)) in c.bounds.iter().enumerate() {
            let ky = &k[yy * c.ksize..yy * c.ksize + n];
            let orow = &mut out[yy * stride..(yy + 1) * stride];
            for (i, o) in orow.iter_mut().enumerate() {
                let mut ss: i64 = 1 << (PRECISION_BITS - 1);
                for (y, &kv) in ky.iter().enumerate() {
                    ss += cur[(ymin + y) * stride + i] as i64 * kv as i64;
                }
                *o = clip8(ss);
            }
        }
        cur = out;
    }
    cur
}

/// Resize a float plane like Pillow's mode "F" (double accumulation, float32 storage).
pub fn resize_f32(src: &[f32], w: usize, h: usize, nw: usize, nh: usize, filter: Filter) -> Vec<f32> {
    if (w, h) == (nw, nh) {
        return src.to_vec();
    }
    let mut cur = src.to_vec();
    let mut cw = w;
    if nw != w {
        let c = precompute(w, nw, filter);
        let mut out = vec![0f32; nw * h];
        for y in 0..h {
            let row = &cur[y * w..(y + 1) * w];
            for (xx, &(xmin, n)) in c.bounds.iter().enumerate() {
                let kx = &c.kk[xx * c.ksize..xx * c.ksize + n];
                let mut ss = 0.0f64;
                for (x, &kv) in kx.iter().enumerate() {
                    ss += row[xmin + x] as f64 * kv;
                }
                out[y * nw + xx] = ss as f32;
            }
        }
        cur = out;
        cw = nw;
    }
    if nh != h {
        let c = precompute(h, nh, filter);
        let mut out = vec![0f32; nh * cw];
        for (yy, &(ymin, n)) in c.bounds.iter().enumerate() {
            let ky = &c.kk[yy * c.ksize..yy * c.ksize + n];
            for x in 0..cw {
                let mut ss = 0.0f64;
                for (y, &kv) in ky.iter().enumerate() {
                    ss += cur[(ymin + y) * cw + x] as f64 * kv;
                }
                out[yy * cw + x] = ss as f32;
            }
        }
        cur = out;
    }
    cur
}

fn muldiv255(a: u32, b: u32) -> u8 {
    let t = a * b + 128;
    (((t >> 8) + t) >> 8) as u8
}

/// Resize RGBA like Pillow: premultiply ("RGBa"), resample, un-premultiply.
pub fn resize_rgba(src: &[u8], w: usize, h: usize, nw: usize, nh: usize, filter: Filter) -> Vec<u8> {
    if (w, h) == (nw, nh) {
        return src.to_vec();
    }
    let mut pre = src.to_vec();
    for p in pre.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = muldiv255(*c as u32, a);
        }
    }
    let mut out = resize_u8(&pre, w, h, 4, nw, nh, filter);
    for p in out.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 && a != 0 {
            for c in &mut p[..3] {
                *c = ((255 * *c as u32) / a).min(255) as u8;
            }
        }
    }
    out
}

/// Pillow `Image.alpha_composite(white, rgba)` -> RGB.
pub fn over_white(rgba: &[u8]) -> Vec<u8> {
    const PB: u32 = 7;
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        let sa = p[3] as u32;
        if sa == 0 {
            out.extend_from_slice(&[255, 255, 255]);
            continue;
        }
        let blend = 255 * (255 - sa);
        let outa255 = sa * 255 + blend;
        let coef1 = sa * 255 * 255 * (1 << PB) / outa255;
        let coef2 = 255 * (1 << PB) - coef1;
        for &c in &p[..3] {
            let tmp = c as u32 * coef1 + 255 * coef2 + (0x80 << PB);
            out.push(((((tmp >> 8) + tmp) >> 8) >> PB) as u8);
        }
    }
    out
}

/// Pillow `ImageFilter.GaussianBlur(radius)` on an 8-bit single-channel image (3 extended box-blur passes).
pub fn gaussian_blur_l(src: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    let r = gaussian_box_radius(radius, 3);
    let mut img = src.to_vec();
    if r == 0.0 {
        return img;
    }
    let mut line = Vec::new();
    for _ in 0..3 {
        for y in 0..h {
            line.clear();
            line.extend_from_slice(&img[y * w..(y + 1) * w]);
            box_line(&line, &mut img[y * w..(y + 1) * w], r);
        }
    }
    let mut col = vec![0u8; h];
    let mut out = vec![0u8; h];
    for _ in 0..3 {
        for x in 0..w {
            for y in 0..h {
                col[y] = img[y * w + x];
            }
            box_line(&col, &mut out, r);
            for y in 0..h {
                img[y * w + x] = out[y];
            }
        }
    }
    img
}

fn gaussian_box_radius(radius: f32, passes: u32) -> f32 {
    let sigma2 = radius * radius / passes as f32;
    let l_big = (12.0 * sigma2 + 1.0).sqrt();
    let l = ((l_big - 1.0) / 2.0).floor();
    let mut a = (2.0 * l + 1.0) * (l * (l + 1.0) - 3.0 * sigma2);
    a /= 6.0 * (sigma2 - (l + 1.0) * (l + 1.0));
    l + a
}

/// One line of Pillow's extended box blur (`ImagingLineBoxBlur8`), edges replicated.
fn box_line(input: &[u8], out: &mut [u8], float_radius: f32) {
    let n = input.len();
    if n == 0 {
        return;
    }
    let radius = float_radius as usize;
    let ww = ((1u32 << 24) as f32 / (float_radius * 2.0 + 1.0)) as u64;
    let fw = ((1u64 << 24) - (radius as u64 * 2 + 1) * ww) / 2;
    let last = n - 1;
    let edge_a = (radius + 1).min(n);
    let px = |i: usize| input[i.min(last)] as i64;
    let mut acc: i64 = px(0) * (radius as i64 + 1);
    for x in 0..edge_a.saturating_sub(1) {
        acc += px(x);
    }
    acc += px(last) * (radius as i64 - edge_a as i64 + 1);
    for (x, o) in out.iter_mut().enumerate().take(n) {
        let sub = if x > radius { x - radius - 1 } else { 0 };
        acc += px(x + radius) - px(sub);
        let bulk = acc as u64 * ww + (px(sub) + px(x + radius + 1)) as u64 * fw;
        *o = ((bulk + (1 << 23)) >> 24) as u8;
    }
}
