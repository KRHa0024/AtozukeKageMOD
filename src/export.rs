//! Saving the results: PNG (lossless, compression level), JPEG / WebP (quality) and layered PSD.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use image::ImageEncoder;

use crate::blend::Mode;
use crate::resample::{self, Filter};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Psd,
}

pub const FORMATS: [Format; 4] = [Format::Png, Format::Jpeg, Format::Webp, Format::Psd];

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Psd => "psd",
        }
    }
}

/// An 8-bit image to save: 1 (gray), 3 (RGB) or 4 (RGBA) channels.
pub struct Pixels<'a> {
    pub w: usize,
    pub h: usize,
    pub channels: usize,
    pub data: &'a [u8],
}

impl Pixels<'_> {
    fn rgb(&self) -> Vec<u8> {
        match self.channels {
            1 => self.data.iter().flat_map(|&v| [v, v, v]).collect(),
            3 => self.data.to_vec(),
            // JPEG has no alpha: flatten over white.
            _ => resample::over_white(self.data),
        }
    }

    fn rgba(&self) -> Vec<u8> {
        match self.channels {
            1 => self.data.iter().flat_map(|&v| [v, v, v, 255]).collect(),
            3 => self.data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            _ => self.data.to_vec(),
        }
    }

    fn opaque(&self) -> bool {
        self.channels != 4 || self.data.chunks_exact(4).all(|p| p[3] == 255)
    }
}

/// `dir/stem_suffix.ext`, numbered when it exists already.
pub fn unique_path(dir: &Path, stem: &str, suffix: &str, ext: &str) -> PathBuf {
    let base = if suffix.is_empty() { stem.to_string() } else { format!("{stem}_{suffix}") };
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base}_{n}.{ext}"));
        n += 1;
    }
    p
}

pub fn save(px: &Pixels, path: &Path, format: Format, png_level: u8, quality: u8) -> Result<(), String> {
    let (w, h) = (px.w as u32, px.h as u32);
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = BufWriter::new(file);
    match format {
        Format::Png => {
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            let level = match png_level {
                0 => CompressionType::Uncompressed,
                l => CompressionType::Level(l.min(9)),
            };
            let enc = PngEncoder::new_with_quality(&mut out, level, FilterType::Adaptive);
            let (data, ct) = match px.channels {
                1 => (px.data.to_vec(), image::ExtendedColorType::L8),
                3 => (px.data.to_vec(), image::ExtendedColorType::Rgb8),
                _ if px.opaque() => (px.rgb(), image::ExtendedColorType::Rgb8),
                _ => (px.data.to_vec(), image::ExtendedColorType::Rgba8),
            };
            enc.write_image(&data, w, h, ct).map_err(|e| e.to_string())?;
        }
        Format::Jpeg => {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            let (data, ct) = if px.channels == 1 {
                (px.data.to_vec(), image::ExtendedColorType::L8)
            } else {
                (px.rgb(), image::ExtendedColorType::Rgb8)
            };
            enc.write_image(&data, w, h, ct).map_err(|e| e.to_string())?;
        }
        Format::Webp => {
            let encoded = if px.opaque() {
                let rgb = px.rgb();
                webp::Encoder::from_rgb(&rgb, w, h).encode(quality.clamp(1, 100) as f32).to_vec()
            } else {
                let rgba = px.rgba();
                webp::Encoder::from_rgba(&rgba, w, h).encode(quality.clamp(1, 100) as f32).to_vec()
            };
            out.write_all(&encoded).map_err(|e| e.to_string())?;
        }
        Format::Psd => return Err("use save_psd".into()),
    }
    out.flush().map_err(|e| e.to_string())
}

pub struct PsdLayer<'a> {
    pub name: &'a str,
    /// RGBA, canvas size.
    pub rgba: Vec<u8>,
    pub mode: Option<Mode>,
    pub opacity: f32,
    pub visible: bool,
}

/// Layered PSD (8-bit RGB, RLE). `merged` is the flattened result shown by viewers without layer support.
pub fn save_psd(path: &Path, w: usize, h: usize, layers: &[PsdLayer], merged: &[u8]) -> Result<(), String> {
    if w > 30000 || h > 30000 {
        return Err("PSD supports up to 30000 px per side".into());
    }
    let mut f = Vec::<u8>::new();
    f.extend_from_slice(b"8BPS");
    be16(&mut f, 1);
    f.extend_from_slice(&[0; 6]);
    be16(&mut f, 3); // merged image channels
    be32(&mut f, h as u32);
    be32(&mut f, w as u32);
    be16(&mut f, 8);
    be16(&mut f, 3); // RGB
    be32(&mut f, 0); // color mode data
    be32(&mut f, 0); // image resources

    // Layer records and channel data.
    let mut records = Vec::new();
    let mut channel_data = Vec::new();
    for layer in layers {
        let mut chans: Vec<(i16, Vec<u8>)> = Vec::new();
        // R, G, B, then alpha, each padded to an even length: valid PSD, and also what simpler readers (Pillow)
        // expect.
        for (id, c) in [(0i16, 0usize), (1, 1), (2, 2), (-1, 3)] {
            let plane: Vec<u8> = layer.rgba.iter().skip(c).step_by(4).copied().collect();
            let mut data = Vec::new();
            be16(&mut data, 1); // RLE
            data.extend(rle_plane(&plane, w, h));
            if data.len() % 2 != 0 {
                data.push(0);
            }
            chans.push((id, data));
        }
        be32(&mut records, 0);
        be32(&mut records, 0);
        be32(&mut records, h as u32);
        be32(&mut records, w as u32);
        be16(&mut records, chans.len() as u16);
        for (id, data) in &chans {
            be16(&mut records, *id as u16);
            be32(&mut records, data.len() as u32);
        }
        records.extend_from_slice(b"8BIM");
        records.extend_from_slice(layer.mode.map(|m| m.psd_key()).unwrap_or(b"norm"));
        records.push((layer.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
        records.push(0); // clipping
        records.push(if layer.visible { 0 } else { 2 }); // flags: bit 1 = hidden
        records.push(0);
        let mut extra = Vec::new();
        be32(&mut extra, 0); // layer mask
        be32(&mut extra, 0); // blending ranges
        let ascii: Vec<u8> = layer.name.bytes().filter(|b| b.is_ascii()).take(255).collect();
        let mut pascal = vec![ascii.len() as u8];
        pascal.extend(ascii);
        while pascal.len() % 4 != 0 {
            pascal.push(0);
        }
        extra.extend(pascal);
        // Unicode layer name (Japanese names).
        let utf16: Vec<u16> = layer.name.encode_utf16().collect();
        let mut luni = Vec::new();
        be32(&mut luni, utf16.len() as u32);
        for u in utf16 {
            be16(&mut luni, u);
        }
        while luni.len() % 4 != 0 {
            luni.push(0);
        }
        extra.extend_from_slice(b"8BIMluni");
        be32(&mut extra, luni.len() as u32);
        extra.extend(luni);
        be32(&mut records, extra.len() as u32);
        records.extend(extra);
        for (_, data) in chans {
            channel_data.extend(data);
        }
    }
    let mut layer_info = Vec::new();
    be16(&mut layer_info, layers.len() as u16);
    layer_info.extend(records);
    layer_info.extend(channel_data);
    if layer_info.len() % 2 != 0 {
        layer_info.push(0);
    }
    let mut lm = Vec::new();
    be32(&mut lm, layer_info.len() as u32);
    lm.extend(layer_info);
    be32(&mut lm, 0); // global layer mask
    be32(&mut f, lm.len() as u32);
    f.extend(lm);

    // Merged image: RLE, all row byte counts first, then the rows of R, G, B.
    be16(&mut f, 1);
    let planes: Vec<Vec<u8>> = (0..3).map(|c| merged.iter().skip(c).step_by(4).copied().collect()).collect();
    let mut counts = Vec::new();
    let mut rows = Vec::new();
    for plane in &planes {
        for y in 0..h {
            let packed = packbits(&plane[y * w..(y + 1) * w]);
            be16(&mut counts, packed.len() as u16);
            rows.extend(packed);
        }
    }
    f.extend(counts);
    f.extend(rows);
    std::fs::write(path, f).map_err(|e| format!("{}: {e}", path.display()))
}

/// Canvas-sized RGBA from any image (resized with Lanczos when needed).
pub fn to_canvas_rgba(px: &Pixels, w: usize, h: usize) -> Vec<u8> {
    let rgba = px.rgba();
    resample::resize_rgba(&rgba, px.w, px.h, w, h, Filter::Lanczos)
}

fn rle_plane(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut counts = Vec::new();
    let mut rows = Vec::new();
    for y in 0..h {
        let packed = packbits(&plane[y * w..(y + 1) * w]);
        be16(&mut counts, packed.len() as u16);
        rows.extend(packed);
    }
    counts.extend(rows);
    counts
}

/// PackBits run-length encoding of one row.
fn packbits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1i16 - run as i16) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        let start = i;
        let mut len = 0;
        while i < row.len() && len < 128 {
            if i + 1 < row.len() && row[i] == row[i + 1] {
                break;
            }
            i += 1;
            len += 1;
        }
        out.push((len - 1) as u8);
        out.extend_from_slice(&row[start..start + len]);
    }
    out
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::packbits;

    fn unpack(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let n = data[i] as i8;
            i += 1;
            if n >= 0 {
                out.extend_from_slice(&data[i..i + n as usize + 1]);
                i += n as usize + 1;
            } else {
                out.extend(std::iter::repeat_n(data[i], (1 - n as i16) as usize));
                i += 1;
            }
        }
        out
    }

    #[test]
    fn packbits_roundtrip() {
        let rows: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 2, 3, 3, 3, 3, 4, 5, 5],
            vec![9; 300],
            (0..1000).map(|i| (i * 7 % 13) as u8).collect(),
            (0..1000).map(|i| (i / 3) as u8).collect(),
        ];
        for r in rows {
            assert_eq!(unpack(&packbits(&r)), r);
        }
    }
}
