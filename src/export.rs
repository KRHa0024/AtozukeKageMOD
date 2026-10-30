//! Saving the results: PNG (lossless, compression level), JPEG / WebP (quality) and layered PSD.

use std::io::Write;
use std::path::{Path, PathBuf};

use image::ImageEncoder;

use crate::blend::{self, Mode, View, AI, ORIGINAL, SHADOW};
use crate::resample::{self, Filter};
use crate::shading::{Image8, Layers};

/// WebP's limit per side.
const WEBP_MAX_SIDE: usize = 16383;
const JPEG_MAX_SIDE: usize = 65535;
const PSD_MAX_SIDE: usize = 30000;
/// WebP at this quality is written lossless.
const WEBP_LOSSLESS_QUALITY: u8 = 100;

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

#[derive(Clone, Copy, Debug)]
pub struct Encoding {
    pub format: Format,
    pub png_level: u8,
    pub quality: u8,
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

fn check_size(w: usize, h: usize, max: usize, format: &str) -> Result<(), String> {
    if w.max(h) > max {
        return Err(format!("{format} supports up to {max} px per side ({w}x{h})"));
    }
    Ok(())
}

/// The file's bytes for a PNG, JPEG or WebP. `icc` is embedded when given (RGB profiles: not for gray pixels).
pub fn encode(px: &Pixels, enc: &Encoding, icc: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let (w, h) = (px.w as u32, px.h as u32);
    let quality = enc.quality.clamp(1, 100);
    let mut out = Vec::new();
    match enc.format {
        Format::Png => {
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            let level = match enc.png_level {
                0 => CompressionType::Uncompressed,
                l => CompressionType::Level(l.min(9)),
            };
            let mut encoder = PngEncoder::new_with_quality(&mut out, level, FilterType::Adaptive);
            if let Some(icc) = icc {
                encoder.set_icc_profile(icc.to_vec()).map_err(|e| e.to_string())?;
            }
            let (data, ct) = match px.channels {
                1 => (px.data.to_vec(), image::ExtendedColorType::L8),
                3 => (px.data.to_vec(), image::ExtendedColorType::Rgb8),
                _ if px.opaque() => (px.rgb(), image::ExtendedColorType::Rgb8),
                _ => (px.data.to_vec(), image::ExtendedColorType::Rgba8),
            };
            encoder.write_image(&data, w, h, ct).map_err(|e| e.to_string())?;
        }
        Format::Jpeg => {
            check_size(px.w, px.h, JPEG_MAX_SIDE, "JPEG")?;
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
            if let Some(icc) = icc {
                encoder.set_icc_profile(icc.to_vec()).map_err(|e| e.to_string())?;
            }
            let (data, ct) = if px.channels == 1 {
                (px.data.to_vec(), image::ExtendedColorType::L8)
            } else {
                (px.rgb(), image::ExtendedColorType::Rgb8)
            };
            encoder.write_image(&data, w, h, ct).map_err(|e| e.to_string())?;
        }
        Format::Webp => {
            check_size(px.w, px.h, WEBP_MAX_SIDE, "WebP")?;
            let opaque = px.opaque();
            let lossless = quality >= WEBP_LOSSLESS_QUALITY;
            let (rgb, rgba);
            let encoder = if opaque {
                rgb = px.rgb();
                webp::Encoder::from_rgb(&rgb, w, h)
            } else {
                rgba = px.rgba();
                webp::Encoder::from_rgba(&rgba, w, h)
            };
            let encoded = encoder.encode_simple(lossless, quality as f32).map_err(|e| format!("WebP: {e:?}"))?.to_vec();
            out = match icc {
                Some(icc) => webp_with_icc(&encoded, icc, px.w, px.h, !opaque)?,
                None => encoded,
            };
        }
        Format::Psd => return Err("use save_psd".into()),
    }
    Ok(out)
}

pub fn save(px: &Pixels, path: &Path, enc: &Encoding, icc: Option<&[u8]>) -> Result<(), String> {
    write_atomic(path, &encode(px, enc, icc)?)
}

/// Through a temporary file, so a failed write never leaves a partial image.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| format!("{}: {e}", path.display()))
}

fn webp_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 != 0 {
        out.push(0);
    }
}

/// Add an ICCP chunk to a WebP file (libwebp's simple encoder writes none): the extended header goes first, then the
/// profile, then the image chunks unchanged.
fn webp_with_icc(file: &[u8], icc: &[u8], w: usize, h: usize, alpha: bool) -> Result<Vec<u8>, String> {
    const ICC_FLAG: u8 = 0x20;
    const ALPHA_FLAG: u8 = 0x10;
    if file.len() < 20 || &file[..4] != b"RIFF" || &file[8..12] != b"WEBP" {
        return Err("WebP: unexpected encoder output".into());
    }
    let mut body = Vec::with_capacity(file.len() + icc.len() + 32);
    let chunks = &file[12..];
    if &chunks[..4] == b"VP8X" {
        let mut header = chunks[8..18].to_vec();
        header[0] |= ICC_FLAG;
        webp_chunk(&mut body, b"VP8X", &header);
        webp_chunk(&mut body, b"ICCP", icc);
        body.extend_from_slice(&chunks[18..]);
    } else {
        let mut header = vec![ICC_FLAG | if alpha { ALPHA_FLAG } else { 0 }, 0, 0, 0];
        header.extend_from_slice(&((w - 1) as u32).to_le_bytes()[..3]);
        header.extend_from_slice(&((h - 1) as u32).to_le_bytes()[..3]);
        webp_chunk(&mut body, b"VP8X", &header);
        webp_chunk(&mut body, b"ICCP", icc);
        body.extend_from_slice(chunks);
    }
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend(body);
    Ok(out)
}

/// One export: the layer panel's state and what to write.
pub struct Job<'a> {
    pub original: &'a Image8,
    /// AI output (model resolution).
    pub shaded: &'a Image8,
    /// AI output at the original's size.
    pub ai_full: &'a [u8],
    pub layers: &'a Layers,
    pub view: View,
    /// Shadow layer, AI output, composite (PSD always holds all three).
    pub targets: [bool; 3],
    pub encoding: Encoding,
    pub dir: &'a Path,
    pub stem: &'a str,
    /// Layer names in the PSD (shadow, AI output, original).
    pub names: [&'a str; 3],
}

/// Written files, and the error that stopped the rest.
#[derive(Debug, Default)]
pub struct Outcome {
    pub written: Vec<PathBuf>,
    pub error: Option<String>,
}

/// File names a job writes in an empty directory (for showing them before exporting).
pub fn planned_names(stem: &str, targets: [bool; 3], format: Format) -> Vec<String> {
    if format == Format::Psd {
        return vec![format!("{stem}_kage.psd")];
    }
    ["shadow", "ai", "shaded"].iter().zip(targets).filter(|(_, on)| *on).map(|(s, _)| format!("{stem}_{s}.{}", format.ext())).collect()
}

pub fn run_job(job: &Job) -> Outcome {
    let mut out = Outcome::default();
    if let Err(e) = write_files(job, &mut out.written) {
        out.error = Some(e);
    }
    out
}

fn write_files(job: &Job, written: &mut Vec<PathBuf>) -> Result<(), String> {
    let Job { original, shaded, ai_full, layers, view, targets, encoding, dir, stem, names } = job;
    let (w, h) = (layers.w, layers.h);
    let format = encoding.format;
    let icc = original.icc.as_ref().map(|p| p.as_slice());
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if format == Format::Psd {
        let psd_layers = [
            PsdLayer { name: names[ORIGINAL], rgba: original.rgba.clone(), mode: None, opacity: 1.0, visible: view.visible[ORIGINAL] },
            PsdLayer { name: names[AI], rgba: ai_full.to_vec(), mode: None, opacity: view.opacity[AI], visible: view.visible[AI] },
            PsdLayer {
                name: names[SHADOW],
                rgba: blend::shadow_rgba(layers, view.mode, &original.rgba),
                mode: Some(view.mode),
                opacity: view.opacity[SHADOW],
                visible: view.visible[SHADOW],
            },
        ];
        let composite = blend::flatten_view(original, ai_full, layers, view);
        let path = unique_path(dir, stem, "kage", "psd");
        save_psd(&path, w, h, &psd_layers, &composite, icc)?;
        written.push(path);
        return Ok(());
    }
    let ext = format.ext();
    if targets[0] {
        let path = unique_path(dir, stem, "shadow", ext);
        if view.mode == Mode::Normal || !original.alpha_is_opaque() {
            let rgba = blend::shadow_rgba(layers, view.mode, &original.rgba);
            save(&Pixels { w, h, channels: 4, data: &rgba }, &path, encoding, None)?;
        } else {
            let channels = if layers.color { 3 } else { 1 };
            save(&Pixels { w, h, channels, data: &layers.multiply }, &path, encoding, None)?;
        }
        written.push(path);
    }
    if targets[1] {
        let path = unique_path(dir, stem, "ai", ext);
        save(&Pixels { w: shaded.w, h: shaded.h, channels: 4, data: &shaded.rgba }, &path, encoding, icc)?;
        written.push(path);
    }
    if targets[2] {
        let path = unique_path(dir, stem, "shaded", ext);
        let composite = blend::flatten_view(original, ai_full, layers, view);
        save(&Pixels { w, h, channels: 4, data: &composite }, &path, encoding, icc)?;
        written.push(path);
    }
    Ok(())
}

pub struct PsdLayer<'a> {
    pub name: &'a str,
    /// RGBA, canvas size.
    pub rgba: Vec<u8>,
    pub mode: Option<Mode>,
    pub opacity: f32,
    pub visible: bool,
}

/// Layered PSD (8-bit RGB, RLE). `merged` is the flattened result shown by viewers without layer support (over white,
/// as it has no alpha).
pub fn save_psd(path: &Path, w: usize, h: usize, layers: &[PsdLayer], merged: &[u8], icc: Option<&[u8]>) -> Result<(), String> {
    check_size(w, h, PSD_MAX_SIDE, "PSD")?;
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
    let mut resources = Vec::new();
    if let Some(icc) = icc {
        resources.extend_from_slice(b"8BIM");
        be16(&mut resources, 1039); // ICC profile
        be16(&mut resources, 0); // empty name
        be32(&mut resources, icc.len() as u32);
        resources.extend_from_slice(icc);
        if icc.len() % 2 != 0 {
            resources.push(0);
        }
    }
    be32(&mut f, resources.len() as u32);
    f.extend(resources);

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
    let flat = resample::over_white(merged);
    let planes: Vec<Vec<u8>> = (0..3).map(|c| flat.iter().skip(c).step_by(3).copied().collect()).collect();
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
    write_atomic(path, &f)
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
