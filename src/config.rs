//! Settings (`<data>/settings.json`) and the app's data folder.
//!
//! The data folder sits next to the exe (portable install); when that is not writable (e.g. Program Files) it
//! falls back to %LOCALAPPDATA%\AtozukeKageMOD.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::export::FORMATS;
use crate::models::MODELS;

/// target/release, target, the repository.
const MODELS_SEARCH_DEPTH: usize = 3;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub lang: String,
    pub model_dir: String,
    pub comfy_dir: String,
    pub keep_loaded: bool,
    pub light_dir: usize,
    pub strength: f32,
    pub bg_shadow: bool,
    pub gray: bool,
    pub ghost: bool,
    pub extra: String,
    pub export_targets: [bool; 3],
    pub export_format: usize,
    pub png_level: u8,
    pub quality: u8,
    pub out_dir: String,
    pub blend: usize,
    /// Canvas surround: 0 light, 1 mid gray, 2 dark.
    pub canvas_shade: usize,
    /// Inner window size in points.
    pub window: Option<[f32; 2]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: "ja".into(),
            model_dir: default_model_dir().display().to_string(),
            comfy_dir: String::new(),
            keep_loaded: true,
            light_dir: 4,
            strength: 1.0,
            bg_shadow: false,
            gray: false,
            ghost: false,
            extra: String::new(),
            export_targets: [true, false, true],
            export_format: 0,
            png_level: 6,
            quality: 92,
            out_dir: default_out_dir().display().to_string(),
            blend: 0,
            canvas_shade: 0,
            window: None,
        }
    }
}

impl Config {
    /// Settings, plus a message when an unreadable file was set aside (as settings.json.bad).
    pub fn load(data: &Path) -> (Self, Option<String>) {
        let path = data.join("settings.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return (Self::default(), None) };
        match serde_json::from_str::<Self>(&text) {
            Ok(cfg) => (cfg.sanitized(), None),
            Err(e) => {
                let _ = std::fs::rename(&path, data.join("settings.json.bad"));
                (Self::default(), Some(format!("settings.json could not be read ({e}); using defaults")))
            }
        }
    }

    /// Values in range, whatever a hand-edited file says: the UI indexes tables with them.
    fn sanitized(mut self) -> Self {
        self.model_dir = self.model_dir.trim().to_string();
        self.comfy_dir = self.comfy_dir.trim().to_string();
        self.light_dir = self.light_dir.min(8);
        self.strength = if self.strength.is_finite() { self.strength.clamp(0.5, 1.5) } else { 1.0 };
        // Index 3 was PSD.
        self.export_format = if self.export_format < FORMATS.len() { self.export_format } else { 0 };
        self.png_level = self.png_level.min(9);
        self.quality = self.quality.clamp(1, 100);
        self.blend = self.blend.min(7);
        self.canvas_shade = self.canvas_shade.min(2);
        self
    }

    pub fn save(&self, data: &Path) -> std::io::Result<()> {
        let tmp = data.join("settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, data.join("settings.json"))
    }
}

/// The bundled `models` directory: next to the exe in a release, in the repository when the exe runs from `target`.
/// Without one it is created next to the exe, or in the data directory when that is not writable.
pub fn default_model_dir() -> PathBuf {
    let exe = exe_dir();
    let bundled = exe.ancestors().take(MODELS_SEARCH_DEPTH).map(|d| d.join("models")).find(|d| d.join(MODELS[0].folder).is_dir());
    bundled.unwrap_or_else(|| {
        let beside = exe.join("models");
        if writable(&beside) {
            beside
        } else {
            data_dir().join("models")
        }
    })
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| ".".into())
}

/// Writable data folder (created).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KAGE_DATA_DIR") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    let local = exe_dir().join("data");
    if writable(&local) {
        return local;
    }
    let fallback = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AtozukeKageMOD");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

fn default_out_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Pictures").join("AtozukeKageMOD"))
        .unwrap_or_else(|| exe_dir().join("output"))
}
