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
    /// A picked file is being copied into the model folder; fraction done.
    Copying(f32),
}

/// Outcome of picking a file for a row, shown under its status.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// The picked file is not the official one (size or SHA-256); nothing was written.
    PickedMismatch,
    PickedFailed(String),
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

/// A free name next to `dest` for a file that is being replaced.
fn aside_path(dest: &Path) -> PathBuf {
    let mut path = dest.with_extension("safetensors.mismatch");
    let mut n = 2;
    while path.exists() {
        path = dest.with_extension(format!("safetensors.mismatch{n}"));
        n += 1;
    }
    path
}

struct State {
    status: [Status; 4],
    notes: [Option<Note>; 4],
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
                notes: std::array::from_fn(|_| None),
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

    pub fn notes(&self) -> [Option<Note>; 4] {
        self.state.lock().unwrap().notes.clone()
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
                s.notes = std::array::from_fn(|_| None);
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

    /// Use a file the user picked for model `i`: verify its size and SHA-256 first (nothing is written for a
    /// file that does not match), then place it as `dir/<folder>/<file>` - a hard link when on the same drive
    /// (no extra space, the original stays where it is), a copy otherwise.
    pub fn place(&self, i: usize, src: PathBuf, dir: PathBuf) {
        let gen = self.generation.load(Ordering::SeqCst);
        self.state.lock().unwrap().running = true;
        let me = self.clone();
        std::thread::spawn(move || {
            let _guard = me.run_lock.lock().unwrap();
            if me.cancelled(gen) {
                return;
            }
            let before = {
                let mut s = me.state.lock().unwrap();
                s.notes[i] = None;
                s.status[i].clone()
            };
            match me.place_file(i, &src, &dir, gen) {
                Ok(()) => me.set(i, Status::Ok),
                Err(note) => {
                    me.set(i, before);
                    if let Some(note) = note {
                        me.state.lock().unwrap().notes[i] = Some(note);
                    }
                }
            }
            if !me.cancelled(gen) {
                me.state.lock().unwrap().running = false;
            }
        });
    }

    /// Err(None) = cancelled.
    fn place_file(&self, i: usize, src: &Path, dir: &Path, gen: u64) -> Result<(), Option<Note>> {
        let m = &MODELS[i];
        let dest = model_path(dir, m);
        let fail = |what: String| {
            self.log.error(format!("Model {}: {what}", m.file));
            Some(Note::PickedFailed(what))
        };
        let (size, _) = stat(src).ok_or_else(|| fail(format!("cannot read {}", src.display())))?;
        if size != m.size {
            self.log.warn(format!("Picked {}: size {size} does not match {} ({}); not used", src.display(), m.file, m.size));
            return Err(Some(Note::PickedMismatch));
        }
        self.log.info(format!("Model {}: checking the picked file {}", m.file, src.display()));
        self.set(i, Status::Checking(0.0));
        let sha = match self.hash(src, i, size, gen) {
            Some(sha) => sha,
            None if self.cancelled(gen) => return Err(None),
            None => return Err(fail(format!("cannot read {}", src.display()))),
        };
        if sha != m.sha256 {
            self.log.warn(format!("Picked {}: SHA-256 {sha} does not match the official file; not used", src.display()));
            return Err(Some(Note::PickedMismatch));
        }
        let same = match (src.canonicalize(), dest.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        let mut cache = self.load_cache();
        let key = dest.display().to_string();
        // A hard link canonicalizes to its own path, so the verified copy is recognized from the record.
        let verified = stat(&dest).is_some_and(|(s, t)| cache.get(&key).is_some_and(|c| c.size == s && c.mtime == t && c.sha256 == m.sha256));
        if !same && !verified {
            let folder = dest.parent().expect("model folder");
            std::fs::create_dir_all(folder).map_err(|e| fail(format!("{}: {e}", folder.display())))?;
            // Keep whatever was there (it did not match) instead of deleting it.
            let aside = dest.exists().then(|| aside_path(&dest));
            if let Some(aside) = &aside {
                std::fs::rename(&dest, aside).map_err(|e| fail(format!("{}: {e}", dest.display())))?;
                self.log.warn(format!("Moved the previous {} aside to {}", m.file, aside.display()));
            }
            if let Err(e) = self.put(i, src, &dest, size, gen) {
                if let Some(aside) = &aside {
                    let _ = std::fs::rename(aside, &dest);
                }
                return Err(e.and_then(|e| fail(format!("copy to {}: {e}", dest.display()))));
            }
        }
        let (dsize, mtime) = stat(&dest).ok_or_else(|| fail(format!("cannot read {}", dest.display())))?;
        cache.insert(key, Seen { size: dsize, mtime, sha256: sha });
        let _ = std::fs::write(&self.cache_path, serde_json::to_vec_pretty(&cache).unwrap_or_default());
        self.log.info(format!("Model {}: match", m.file));
        Ok(())
    }

    /// Hard link when the drive allows it, otherwise a copy that is verified again: only a matching file may stay.
    fn put(&self, i: usize, src: &Path, dest: &Path, size: u64, gen: u64) -> Result<(), Option<String>> {
        let m = &MODELS[i];
        if std::fs::hard_link(src, dest).is_ok() {
            self.log.info(format!("Model {}: linked {} -> {}", m.file, src.display(), dest.display()));
            return Ok(());
        }
        self.copy(i, src, dest, size, gen)?;
        self.set(i, Status::Checking(0.0));
        if self.hash(dest, i, size, gen).as_deref() != Some(m.sha256) {
            let _ = std::fs::remove_file(dest);
            return if self.cancelled(gen) { Err(None) } else { Err(Some("the copy does not match".into())) };
        }
        self.log.info(format!("Model {}: copied {} -> {}", m.file, src.display(), dest.display()));
        Ok(())
    }

    /// Copy through a temporary file, so a cancelled or failed copy never leaves a partial model file.
    fn copy(&self, i: usize, src: &Path, dest: &Path, size: u64, gen: u64) -> Result<(), Option<String>> {
        use std::io::Write;
        let tmp = dest.with_extension("safetensors.part");
        let result = (|| -> Result<(), Option<String>> {
            let mut r = std::fs::File::open(src).map_err(|e| Some(e.to_string()))?;
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| Some(e.to_string()))?);
            let mut buf = vec![0u8; 8 << 20];
            let mut done = 0u64;
            loop {
                if self.cancelled(gen) {
                    return Err(None);
                }
                let n = r.read(&mut buf).map_err(|e| Some(e.to_string()))?;
                if n == 0 {
                    break;
                }
                w.write_all(&buf[..n]).map_err(|e| Some(e.to_string()))?;
                done += n as u64;
                self.set(i, Status::Copying(done as f32 / size.max(1) as f32));
            }
            w.flush().map_err(|e| Some(e.to_string()))?;
            drop(w);
            std::fs::rename(&tmp, dest).map_err(|e| Some(e.to_string()))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
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
