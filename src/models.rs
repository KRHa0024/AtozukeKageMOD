//! The four model files and their verification.
//!
//! Nothing is downloaded. The user places the files; each is accepted only when its name, size and SHA-256 match
//! the official release (hashes = the Hugging Face LFS object ids of Comfy-Org/Qwen-Image-2.1 and
//! Comfy-Org/BiRefNet). A verified file is remembered by size + modification time, so the 17.7 GB are hashed once.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::log::Log;

pub struct ModelFile {
    pub file: &'static str,
    /// Sub-folder of the model folder (ComfyUI model type).
    pub folder: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub source: &'static str,
}

pub const MODELS: [ModelFile; 4] = [
    ModelFile {
        file: "qwen_image_2.1_int8_convrot.safetensors",
        folder: "diffusion_models",
        size: 7_256_783_064,
        sha256: "cb74113cb03faecd79611b01fd7fd642f0aa60d6f0b95086abee214d75eaa57d",
        source: "https://huggingface.co/Comfy-Org/Qwen-Image-2.1/blob/main/diffusion_models/qwen_image_2.1_int8_convrot.safetensors",
    },
    ModelFile {
        file: "qwen3vl_8b_int8_convrot.safetensors",
        folder: "text_encoders",
        size: 9_350_798_360,
        sha256: "8bfd0f6e12abf2d2d697ecc888e5e90b0d6741d6708f05799f53afa560452e8f",
        source: "https://huggingface.co/Comfy-Org/Qwen-Image-2.1/blob/main/text_encoders/qwen3vl_8b_int8_convrot.safetensors",
    },
    ModelFile {
        file: "qwen_image_2.1_vae_bf16.safetensors",
        folder: "vae",
        size: 675_509_688,
        sha256: "bb21f7473051e1ac368515dd3f2e15cd44d7a11748ee8823e1ddca3e4876b7c9",
        source: "https://huggingface.co/Comfy-Org/Qwen-Image-2.1/blob/main/vae/qwen_image_2.1_vae_bf16.safetensors",
    },
    ModelFile {
        file: "birefnet.safetensors",
        folder: "background_removal",
        size: 444_473_596,
        sha256: "9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154",
        source: "https://huggingface.co/Comfy-Org/BiRefNet/blob/main/background_removal/birefnet.safetensors",
    },
];

pub fn total_bytes() -> u64 {
    MODELS.iter().map(|m| m.size).sum()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Unchecked,
    /// Hashing; fraction done.
    Checking(f32),
    Ok,
    Missing,
    /// Wrong size or hash (the file is not loaded).
    Mismatch,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
struct Seen {
    size: u64,
    mtime: u128,
    sha256: String,
}

fn stat(path: &Path) -> Option<(u64, u128)> {
    let m = std::fs::metadata(path).ok()?;
    if !m.is_file() {
        return None;
    }
    let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((m.len(), mtime))
}

pub fn model_path(dir: &Path, m: &ModelFile) -> PathBuf {
    dir.join(m.folder).join(m.file)
}

struct State {
    status: [Status; 4],
    dir: PathBuf,
    running: bool,
}

#[derive(Clone)]
pub struct Verifier {
    state: Arc<Mutex<State>>,
    cache_path: PathBuf,
    /// Bumped by every start(); a run whose generation is no longer current stops.
    generation: Arc<AtomicU64>,
    run_lock: Arc<Mutex<()>>,
    log: Log,
}

impl Verifier {
    pub fn new(data: &Path, log: Log) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                status: std::array::from_fn(|_| Status::Unchecked),
                dir: PathBuf::new(),
                running: false,
            })),
            cache_path: data.join("verified.json"),
            generation: Arc::new(AtomicU64::new(0)),
            run_lock: Arc::new(Mutex::new(())),
            log,
        }
    }

    pub fn statuses(&self) -> [Status; 4] {
        self.state.lock().unwrap().status.clone()
    }

    pub fn running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    pub fn all_ok(&self) -> bool {
        self.statuses().iter().all(|s| *s == Status::Ok)
    }

    fn load_cache(&self) -> HashMap<String, Seen> {
        std::fs::read_to_string(&self.cache_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Verify the files in `dir` in the background (a running check of another folder is abandoned).
    pub fn start(&self, dir: PathBuf) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.lock().unwrap().running = true;
        let me = self.clone();
        std::thread::spawn(move || {
            let _guard = me.run_lock.lock().unwrap();
            if me.cancelled(gen) {
                return;
            }
            {
                let mut s = me.state.lock().unwrap();
                s.running = true;
                s.dir = dir.clone();
                s.status = std::array::from_fn(|_| Status::Unchecked);
            }
            me.run(&dir, gen);
            if !me.cancelled(gen) {
                me.state.lock().unwrap().running = false;
            }
        });
    }

    fn cancelled(&self, gen: u64) -> bool {
        self.generation.load(Ordering::SeqCst) != gen
    }

    fn set(&self, i: usize, st: Status) {
        self.state.lock().unwrap().status[i] = st;
    }

    fn run(&self, dir: &Path, gen: u64) {
        let mut cache = self.load_cache();
        let mut ok = 0;
        for (i, m) in MODELS.iter().enumerate() {
            let path = model_path(dir, m);
            let key = path.display().to_string();
            if self.cancelled(gen) {
                return;
            }
            let Some((size, mtime)) = stat(&path) else {
                self.set(i, Status::Missing);
                continue;
            };
            if size != m.size {
                self.log.warn(format!("Model {}: size {size} does not match the official file ({})", m.file, m.size));
                self.set(i, Status::Mismatch);
                continue;
            }
            let sha = match cache.get(&key) {
                Some(seen) if seen.size == size && seen.mtime == mtime => seen.sha256.clone(),
                _ => {
                    self.set(i, Status::Checking(0.0));
                    self.log.info(format!("Model {}: computing SHA-256 (first time only)", m.file));
                    match self.hash(&path, i, size, gen) {
                        Some(sha) => {
                            cache.insert(key.clone(), Seen { size, mtime, sha256: sha.clone() });
                            let _ = std::fs::write(&self.cache_path, serde_json::to_vec_pretty(&cache).unwrap_or_default());
                            sha
                        }
                        None if self.cancelled(gen) => return,
                        None => {
                            self.log.error(format!("Model {}: could not read the file", m.file));
                            self.set(i, Status::Mismatch);
                            continue;
                        }
                    }
                }
            };
            if sha == m.sha256 {
                ok += 1;
                self.set(i, Status::Ok);
            } else {
                self.log.warn(format!("Model {}: SHA-256 {sha} does not match the official file; it will not be loaded", m.file));
                self.set(i, Status::Mismatch);
            }
        }
        self.log.info(format!("Model check: {ok}/{} match ({})", MODELS.len(), dir.display()));
    }

    fn hash(&self, path: &Path, i: usize, size: u64, gen: u64) -> Option<String> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 8 << 20];
        let mut done = 0u64;
        loop {
            if self.cancelled(gen) {
                return None;
            }
            let n = f.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            done += n as u64;
            self.set(i, Status::Checking(done as f32 / size.max(1) as f32));
        }
        Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Right before a run: the files must still be the verified ones (same size and modification time).
    pub fn still_valid(&self, dir: &Path) -> bool {
        let cache = self.load_cache();
        MODELS.iter().all(|m| {
            let path = model_path(dir, m);
            match (stat(&path), cache.get(&path.display().to_string())) {
                (Some((size, mtime)), Some(seen)) => seen.size == size && seen.mtime == mtime && seen.sha256 == m.sha256,
                _ => false,
            }
        })
    }
}
