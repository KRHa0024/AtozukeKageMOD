//! Saving the results: PNG (lossless, compression level), JPEG / WebP (quality).

use std::io::Write;
use std::path::{Path, PathBuf};

use image::ImageEncoder;

use crate::blend::{self, Mode, View};
use crate::resample::{self, Filter};
use crate::shading::{Image8, Layers};

/// WebP's limit per side.
const WEBP_MAX_SIDE: usize = 16383;
const JPEG_MAX_SIDE: usize = 65535;
/// WebP at this quality is written lossless.
const WEBP_LOSSLESS_QUALITY: u8 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

pub const FORMATS: [Format; 3] = [Format::Png, Format::Jpeg, Format::Webp];

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
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

/// The file's bytes. `icc` is embedded when given (RGB profiles: not for gray pixels).
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
    /// Shadow layer, AI output, composite.
    pub targets: [bool; 3],
    pub encoding: Encoding,
    pub dir: &'a Path,
    pub stem: &'a str,
}

/// Written files, and the error that stopped the rest.
#[derive(Debug, Default)]
pub struct Outcome {
    pub written: Vec<PathBuf>,
    pub error: Option<String>,
}

/// File names a job writes in an empty directory (for showing them before exporting).
pub fn planned_names(stem: &str, targets: [bool; 3], format: Format) -> Vec<String> {
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
    let Job { original, shaded, ai_full, layers, view, targets, encoding, dir, stem } = job;
    let (w, h) = (layers.w, layers.h);
    let format = encoding.format;
    let icc = original.icc.as_ref().map(|p| p.as_slice());
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let ext = format.ext();
    if targets[0] {
        let path = unique_path(dir, stem, "shadow", ext);
        if view.mode == Mode::Normal || !original.alpha_is_opaque() || view.shadow_offset != (0, 0) {
            let rgba = blend::shadow_rgba(layers, view.mode, &original.rgba, view.shadow_offset);
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

/// Canvas-sized RGBA from any image (resized with Lanczos when needed).
pub fn to_canvas_rgba(px: &Pixels, w: usize, h: usize) -> Vec<u8> {
    let rgba = px.rgba();
    resample::resize_rgba(&rgba, px.w, px.h, w, h, Filter::Lanczos)
}
