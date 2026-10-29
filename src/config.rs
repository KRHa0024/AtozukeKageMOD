//! Settings (`<data>/settings.json`) and the app's data folder.
//!
//! The data folder sits next to the exe (portable install); when that is not writable (e.g. Program Files) it
//! falls back to %LOCALAPPDATA%\AtozukeKageMOD.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
}

impl Default for Config {
    fn default() -> Self {
        let exe_dir = exe_dir();
        Self {
            lang: "ja".into(),
            model_dir: exe_dir.join("models").display().to_string(),
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
        }
    }
}

impl Config {
    /// Settings, plus a message when an unreadable file was set aside (as settings.json.bad).
    pub fn load(data: &Path) -> (Self, Option<String>) {
        let path = data.join("settings.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return (Self::default(), None) };
        match serde_json::from_str(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => {
                let _ = std::fs::rename(&path, data.join("settings.json.bad"));
                (Self::default(), Some(format!("settings.json could not be read ({e}); using defaults")))
            }
        }
    }

    pub fn save(&self, data: &Path) -> std::io::Result<()> {
        let tmp = data.join("settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, data.join("settings.json"))
    }
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
