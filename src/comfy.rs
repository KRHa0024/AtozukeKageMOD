//! ComfyUI: finds the user's install, runs a private instance in the background (own port, own input/output
//! folders, only our model folder, no custom nodes) and talks to it over HTTP + WebSocket.
//!
//! The process is put in a Windows job object that kills it when the app exits, even on a crash.

use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::log::Log;

/// Core nodes the workflows need (ComfyUI 0.40 or newer).
pub const REQUIRED_NODES: [&str; 4] = ["QwenImage21Cache", "TextEncodeQwenImage21", "LoadBackgroundRemovalModel", "RemoveBackground"];
const START_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// No folder chosen yet.
    NotSet,
    /// The folder has no ComfyUI (main.py + Python) in it.
    NotFound,
    Starting,
    Ready { version: String },
    /// Too old: lacks nodes the workflows use.
    MissingNodes(Vec<String>),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Install {
    pub python: PathBuf,
    pub main_py: PathBuf,
    pub version: Option<String>,
}

/// Accepts the portable release folder (python_embeded + ComfyUI\main.py), its ComfyUI sub-folder, or a git
/// checkout with a venv.
pub fn detect(dir: &Path) -> Option<Install> {
    let candidates = [
        (dir.join("python_embeded").join("python.exe"), dir.join("ComfyUI")),
        (dir.parent().map(|p| p.join("python_embeded").join("python.exe")).unwrap_or_default(), dir.to_path_buf()),
        (dir.join(".venv").join("Scripts").join("python.exe"), dir.to_path_buf()),
        (dir.join("venv").join("Scripts").join("python.exe"), dir.to_path_buf()),
    ];
    candidates.into_iter().find(|(py, root)| py.is_file() && root.join("main.py").is_file()).map(|(python, root)| {
        let version = std::fs::read_to_string(root.join("comfyui_version.py"))
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("__version__")).map(|l| l.split('"').nth(1).unwrap_or("").to_string()));
        Install { python, main_py: root.join("main.py"), version }
    })
}

struct Proc {
    child: Child,
    #[cfg(windows)]
    _job: Option<KillOnClose>,
}

struct Inner {
    state: State,
    port: u16,
    proc: Option<Proc>,
}

#[derive(Clone)]
pub struct Comfy {
    inner: Arc<Mutex<Inner>>,
    generation: Arc<AtomicU64>,
    data: PathBuf,
    log: Log,
}

impl Comfy {
    pub fn new(data: &Path, log: Log) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner { state: State::NotSet, port: 0, proc: None })),
            generation: Arc::new(AtomicU64::new(0)),
            data: data.join("comfy"),
            log,
        }
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn input_dir(&self) -> PathBuf {
        self.data.join("input")
    }

    pub fn output_dir(&self) -> PathBuf {
        self.data.join("output")
    }

    pub fn log_path(&self) -> PathBuf {
        self.log.dir().join("comfyui.log")
    }

    fn set_state(&self, gen: u64, state: State) {
        if self.generation.load(Ordering::SeqCst) == gen {
            self.inner.lock().unwrap().state = state;
        }
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let proc = self.inner.lock().unwrap().proc.take();
        if let Some(mut p) = proc {
            let _ = p.child.kill();
            let _ = p.child.wait();
            self.log.info("ComfyUI stopped");
        }
    }

    /// (Re)start the private instance for `comfy_dir`, reading models only from `model_dir`.
    pub fn start(&self, comfy_dir: &str, model_dir: &Path) {
        self.stop();
        let gen = self.generation.load(Ordering::SeqCst);
        if comfy_dir.trim().is_empty() {
            self.set_state(gen, State::NotSet);
            return;
        }
        let Some(install) = detect(Path::new(comfy_dir.trim())) else {
            self.log.warn(format!("No ComfyUI found in {comfy_dir}"));
            self.set_state(gen, State::NotFound);
            return;
        };
        match self.spawn(&install, model_dir) {
            Ok((proc, port)) => {
                {
                    let mut g = self.inner.lock().unwrap();
                    g.proc = Some(proc);
                    g.port = port;
                    g.state = State::Starting;
                }
                self.log.info(format!(
                    "Launching ComfyUI {} from {} (port {port})",
                    install.version.as_deref().unwrap_or("?"),
                    comfy_dir.trim()
                ));
                let me = self.clone();
                std::thread::spawn(move || me.watch(gen, port));
            }
            Err(e) => {
                self.log.error(format!("Could not launch ComfyUI: {e}"));
                self.set_state(gen, State::Failed(e));
            }
        }
    }

    fn spawn(&self, install: &Install, model_dir: &Path) -> Result<(Proc, u16), String> {
        let (input, output, temp, user) =
            (self.input_dir(), self.output_dir(), self.data.join("temp"), self.data.join("user"));
        for d in [&input, &output, &temp, &user] {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        // Only the verified files' folders are registered, and they take precedence over ComfyUI's own models.
        let base = model_dir.display().to_string().replace('\\', "/");
        let yaml = format!(
            "kage:\n  base_path: \"{base}/\"\n  is_default: true\n  diffusion_models: diffusion_models/\n  text_encoders: text_encoders/\n  vae: vae/\n  background_removal: background_removal/\n"
        );
        let yaml_path = self.data.join("extra_model_paths.yaml");
        std::fs::write(&yaml_path, yaml).map_err(|e| e.to_string())?;
        let port = free_port().ok_or("no free local port")?;
        let log_path = self.log_path();
        let out = std::fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&install.python);
        cmd.arg("-s")
            .arg(&install.main_py)
            .args(["--listen", "127.0.0.1", "--port", &port.to_string(), "--disable-auto-launch"])
            .args(["--disable-all-custom-nodes", "--disable-api-nodes"])
            .arg("--extra-model-paths-config")
            .arg(&yaml_path)
            .arg("--input-directory")
            .arg(&input)
            .arg("--output-directory")
            .arg(&output)
            .arg("--temp-directory")
            .arg(&temp)
            .arg("--user-directory")
            .arg(&user)
            .current_dir(install.main_py.parent().unwrap_or(Path::new(".")))
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| format!("{}: {e}", install.python.display()))?;
        #[cfg(windows)]
        let job = KillOnClose::new().inspect(|j| j.assign(&child));
        Ok((Proc { child, #[cfg(windows)] _job: job }, port))
    }

    fn exited(&self) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        let p = g.proc.as_mut()?;
        match p.child.try_wait() {
            Ok(Some(status)) => Some(format!("exit code {}", status.code().unwrap_or(-1))),
            _ => None,
        }
    }

    fn log_tail(&self) -> String {
        let mut s = String::new();
        if let Ok(mut f) = std::fs::File::open(self.log_path()) {
            let _ = f.read_to_string(&mut s);
        }
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(3)..].join(" / ")
    }

    /// Wait until the server answers, check the nodes, then keep an eye on the process.
    fn watch(&self, gen: u64, port: u16) {
        let base = format!("http://127.0.0.1:{port}");
        let started = Instant::now();
        loop {
            if self.generation.load(Ordering::SeqCst) != gen {
                return;
            }
            if let Some(code) = self.exited() {
                let tail = self.log_tail();
                self.log.error(format!("ComfyUI exited while starting ({code}): {tail}"));
                self.set_state(gen, State::Failed(format!("{code}: {tail}")));
                return;
            }
            if let Ok(stats) = get_json(&format!("{base}/system_stats"), Duration::from_secs(2)) {
                let version = stats["system"]["comfyui_version"].as_str().unwrap_or("?").to_string();
                let missing: Vec<String> = REQUIRED_NODES
                    .iter()
                    .filter(|n| {
                        get_json(&format!("{base}/object_info/{n}"), Duration::from_secs(10))
                            .map(|v| v.get(**n).is_none())
                            .unwrap_or(true)
                    })
                    .map(|n| n.to_string())
                    .collect();
                if missing.is_empty() {
                    let gpu = stats["devices"][0]["name"].as_str().unwrap_or("?").to_string();
                    self.log.info(format!(
                        "ComfyUI ready (v{version}, port {port}, {gpu}) in {:.1} s",
                        started.elapsed().as_secs_f32()
                    ));
                    self.set_state(gen, State::Ready { version });
                } else {
                    self.log.error(format!("ComfyUI v{version} lacks nodes {}; update ComfyUI", missing.join(", ")));
                    self.set_state(gen, State::MissingNodes(missing));
                    return;
                }
                break;
            }
            if started.elapsed() > START_TIMEOUT {
                self.log.error("ComfyUI did not answer within 5 minutes");
                self.set_state(gen, State::Failed("timeout".into()));
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        while self.generation.load(Ordering::SeqCst) == gen {
            if let Some(code) = self.exited() {
                let tail = self.log_tail();
                self.log.error(format!("ComfyUI stopped unexpectedly ({code}): {tail}"));
                self.set_state(gen, State::Failed(format!("{code}: {tail}")));
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// Base URL when ready.
    pub fn url(&self) -> Option<String> {
        let g = self.inner.lock().unwrap();
        matches!(g.state, State::Ready { .. }).then(|| format!("http://127.0.0.1:{}", g.port))
    }

    pub fn port(&self) -> u16 {
        self.inner.lock().unwrap().port
    }

    /// Unload models and free GPU memory.
    pub fn free(&self) {
        if let Some(url) = self.url() {
            let _ = post_json(&format!("{url}/free"), &json!({"unload_models": true, "free_memory": true}));
        }
    }

    /// Queue `graph` and wait for it; returns the image files written by `output_node`.
    /// `on_progress(value, max)` gets sampler steps. Stops (interrupting ComfyUI) when `cancel` is set.
    pub fn run(
        &self,
        graph: &Value,
        output_node: &str,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(u32, u32),
    ) -> Result<Vec<PathBuf>, RunError> {
        let url = self.url().ok_or_else(|| RunError::Failed("ComfyUI is not ready".into()))?;
        let client_id = random_hex();
        let port = self.port();
        // Connect first so no progress message is missed.
        let mut ws = TcpStream::connect(("127.0.0.1", port)).ok().and_then(|tcp| {
            tcp.set_read_timeout(Some(Duration::from_millis(250))).ok()?;
            tungstenite::client(format!("ws://127.0.0.1:{port}/ws?clientId={client_id}"), tcp).ok().map(|(ws, _)| ws)
        });
        let resp = post_json(&format!("{url}/prompt"), &json!({"prompt": graph, "client_id": client_id}))
            .map_err(RunError::Failed)?;
        let prompt_id = resp["prompt_id"].as_str().ok_or_else(|| RunError::Failed(format!("unexpected answer: {resp}")))?.to_string();
        let mut last_poll = Instant::now();
        loop {
            if cancel.load(Ordering::SeqCst) {
                let _ = post_json(&format!("{url}/queue"), &json!({"delete": [prompt_id]}));
                let _ = post_json(&format!("{url}/interrupt"), &json!({"prompt_id": prompt_id}));
                return Err(RunError::Cancelled);
            }
            if let Some(code) = self.exited() {
                return Err(RunError::Failed(format!("ComfyUI stopped ({code})")));
            }
            let mut done = false;
            match ws.as_mut().map(|w| w.read()) {
                Some(Ok(tungstenite::Message::Text(text))) => {
                    let msg: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    let data = &msg["data"];
                    if data["prompt_id"].as_str() == Some(prompt_id.as_str()) {
                        match msg["type"].as_str() {
                            Some("progress") => {
                                on_progress(data["value"].as_u64().unwrap_or(0) as u32, data["max"].as_u64().unwrap_or(0) as u32)
                            }
                            Some("executing") if data["node"].is_null() => done = true,
                            Some("execution_success") => done = true,
                            Some("execution_error") => {
                                let m = data["exception_message"].as_str().unwrap_or("error").trim().to_string();
                                let node = data["node_type"].as_str().unwrap_or("?");
                                return Err(RunError::Failed(format!("{node}: {m}")));
                            }
                            Some("execution_interrupted") => return Err(RunError::Cancelled),
                            _ => {}
                        }
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(tungstenite::Error::Io(e)))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Some(Err(_)) => ws = None, // fall back to polling the history
                None => std::thread::sleep(Duration::from_millis(250)),
            }
            if !done && (ws.is_none() || last_poll.elapsed() > Duration::from_secs(5)) {
                last_poll = Instant::now();
                done = get_json(&format!("{url}/history/{prompt_id}"), Duration::from_secs(5))
                    .map(|h| h.get(&prompt_id).is_some())
                    .unwrap_or(false);
            }
            if done {
                break;
            }
        }
        let history = get_json(&format!("{url}/history/{prompt_id}"), Duration::from_secs(10)).map_err(RunError::Failed)?;
        let entry = &history[&prompt_id];
        if entry["status"]["status_str"].as_str() == Some("error") {
            let msg = entry["status"]["messages"]
                .as_array()
                .and_then(|m| m.iter().find(|m| m[0] == "execution_error"))
                .and_then(|m| m[1]["exception_message"].as_str())
                .unwrap_or("execution error");
            return Err(RunError::Failed(msg.trim().to_string()));
        }
        let images = entry["outputs"][output_node]["images"].as_array().cloned().unwrap_or_default();
        if images.is_empty() {
            return Err(RunError::Failed("the workflow produced no image".into()));
        }
        Ok(images
            .iter()
            .map(|i| {
                let mut p = self.output_dir();
                if let Some(sub) = i["subfolder"].as_str().filter(|s| !s.is_empty()) {
                    p = p.join(sub);
                }
                p.join(i["filename"].as_str().unwrap_or(""))
            })
            .collect())
    }
}

#[derive(Debug)]
pub enum RunError {
    Cancelled,
    Failed(String),
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout(timeout).build()
}

fn get_json(url: &str, timeout: Duration) -> Result<Value, String> {
    agent(timeout).get(url).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())
}

fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    match agent(Duration::from_secs(30)).post(url).send_json(body) {
        Ok(r) => {
            let text = r.into_string().map_err(|e| e.to_string())?;
            Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
        }
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let msg = body["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| body.to_string());
            let details = body["node_errors"]
                .as_object()
                .map(|n| {
                    n.values()
                        .flat_map(|v| v["errors"].as_array().cloned().unwrap_or_default())
                        .filter_map(|e| e["details"].as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            Err(format!("HTTP {code}: {msg} {details}").trim().to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok().map(|a| a.port())
}

/// Random 64-bit value without a RNG crate (the std hasher is randomly keyed per process).
pub fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

fn random_hex() -> String {
    format!("{:016x}{:016x}", random_u64(), random_u64())
}

#[cfg(windows)]
struct KillOnClose(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for KillOnClose {}

#[cfg(windows)]
impl KillOnClose {
    fn new() -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            Some(Self(job))
        }
    }

    fn assign(&self, child: &Child) {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, child.as_raw_handle() as _);
        }
    }
}

#[cfg(windows)]
impl Drop for KillOnClose {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
