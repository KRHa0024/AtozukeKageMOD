//! `Comfy::run` against a fake ComfyUI (HTTP + WebSocket on 127.0.0.1) that follows the message order of the real
//! server: `execution_success` is sent before the run is in the history, `executing` with node null after it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kage::comfy::{Comfy, Failure, RunError};
use kage::log::Log;
use serde_json::{json, Value};

const ID: &str = "p";

#[derive(Clone)]
enum Scenario {
    /// Real order: execution_success, `pause_ms`, history entry, executing(node null).
    Success { pause_ms: u64 },
    /// `execution_error` over the WebSocket.
    WsError(&'static str),
    /// Only the history says it failed (after executing(node null)).
    HistoryError(&'static str),
    /// Progress forever (until the client goes away).
    Long,
    /// The WebSocket is refused (404); the run enters the history after `after_ms`.
    NoWs { after_ms: u64 },
}

struct Fake {
    scenario: Scenario,
    history: Mutex<HashMap<String, Value>>,
    ws: Mutex<HashMap<String, Sender<String>>>,
    /// (method, path, body) of every plain HTTP request.
    requests: Mutex<Vec<(String, String, String)>>,
}

impl Fake {
    fn requested(&self, method: &str, path: &str) -> Vec<String> {
        self.requests.lock().unwrap().iter().filter(|r| r.0 == method && r.1 == path).map(|r| r.2.clone()).collect()
    }

    /// False when the client is gone.
    fn send(&self, cid: &str, v: Value) -> bool {
        match self.ws.lock().unwrap().get(cid) {
            Some(tx) => tx.send(v.to_string()).is_ok(),
            None => false,
        }
    }
}

fn history_entry() -> Value {
    json!({"outputs": {"9": {"images": [{"filename": "x.png", "subfolder": "kage", "type": "output"}]}}, "status": {"status_str": "success", "completed": true, "messages": []}})
}

fn job(fake: Arc<Fake>, cid: String) {
    let wait_for_ws = !matches!(fake.scenario, Scenario::NoWs { .. });
    let start = Instant::now();
    while wait_for_ws && !fake.ws.lock().unwrap().contains_key(&cid) {
        assert!(start.elapsed() < Duration::from_secs(5), "the client never opened the WebSocket");
        std::thread::sleep(Duration::from_millis(5));
    }
    let ok = |v: Value| fake.send(&cid, v);
    ok(json!({"type": "execution_start", "data": {"prompt_id": ID}}));
    ok(json!({"type": "executing", "data": {"node": "7", "prompt_id": ID}}));
    match fake.scenario.clone() {
        Scenario::Long => {
            // A message of another prompt must not count as progress of ours.
            for i in (1..=25).cycle() {
                if !ok(json!({"type": "progress", "data": {"value": i, "max": 25, "prompt_id": ID, "node": "7"}})) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Scenario::NoWs { after_ms } => {
            std::thread::sleep(Duration::from_millis(after_ms));
            fake.history.lock().unwrap().insert(ID.into(), history_entry());
        }
        Scenario::WsError(message) => {
            ok(json!({"type": "execution_error", "data": {
                "prompt_id": ID, "node_id": "7", "node_type": "KSampler",
                "exception_type": "RuntimeError", "exception_message": message
            }}));
        }
        Scenario::HistoryError(message) => {
            let entry = json!({"outputs": {}, "status": {"status_str": "error", "completed": false, "messages": [
                ["execution_start", {"prompt_id": ID}],
                ["execution_error", {"prompt_id": ID, "exception_message": message}]
            ]}});
            fake.history.lock().unwrap().insert(ID.into(), entry);
            ok(json!({"type": "executing", "data": {"node": null, "prompt_id": ID}}));
        }
        Scenario::Success { pause_ms } => {
            // Another prompt's progress is ignored.
            ok(json!({"type": "progress", "data": {"value": 99, "max": 99, "prompt_id": "other", "node": "7"}}));
            for i in 1..=5 {
                ok(json!({"type": "progress", "data": {"value": i, "max": 5, "prompt_id": ID, "node": "7"}}));
                std::thread::sleep(Duration::from_millis(10));
            }
            ok(json!({"type": "execution_success", "data": {"prompt_id": ID}}));
            std::thread::sleep(Duration::from_millis(pause_ms));
            fake.history.lock().unwrap().insert(ID.into(), history_entry());
            ok(json!({"type": "executing", "data": {"node": null, "prompt_id": ID}}));
        }
    }
}

fn http_reply(s: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.flush();
}

fn handle(mut s: TcpStream, fake: Arc<Fake>) {
    // Look at the request without consuming it, so a WebSocket request can go to tungstenite untouched.
    let mut peek = [0u8; 4096];
    let mut tries = 0;
    let head_len = loop {
        let n = match s.peek(&mut peek) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if let Some(p) = peek[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        tries += 1;
        if tries > 1000 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let head = String::from_utf8_lossy(&peek[..head_len]).to_string();
    let first = head.lines().next().unwrap_or("").to_string();
    let mut it = first.split(' ');
    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());

    if path.starts_with("/ws") {
        if matches!(fake.scenario, Scenario::NoWs { .. }) {
            fake.requests.lock().unwrap().push((method, path, String::new()));
            let mut sink = vec![0u8; head_len];
            let _ = s.read_exact(&mut sink);
            http_reply(&mut s, "404 Not Found", "{}");
            return;
        }
        let cid = path.split("clientId=").nth(1).unwrap_or("").to_string();
        let Ok(mut ws) = tungstenite::accept(s) else { return };
        let (tx, rx) = channel::<String>();
        fake.ws.lock().unwrap().insert(cid, tx);
        while let Ok(m) = rx.recv() {
            if ws.send(tungstenite::Message::Text(m.into())).is_err() {
                break;
            }
        }
        return;
    }

    let mut content_length = 0usize;
    for l in head.lines().skip(1) {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut raw = vec![0u8; head_len + content_length];
    if s.read_exact(&mut raw).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&raw[head_len..]).to_string();
    fake.requests.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
    match (method.as_str(), path.as_str()) {
        ("POST", "/prompt") => {
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let cid = v["client_id"].as_str().unwrap_or("").to_string();
            http_reply(&mut s, "200 OK", &json!({"prompt_id": ID, "number": 1, "node_errors": {}}).to_string());
            let fake = fake.clone();
            std::thread::spawn(move || job(fake, cid));
        }
        ("GET", p) if p == format!("/history/{ID}") => {
            let h = fake.history.lock().unwrap();
            let out = match h.get(ID) {
                Some(e) => json!({ ID: e }),
                None => json!({}),
            };
            drop(h);
            http_reply(&mut s, "200 OK", &out.to_string());
        }
        _ => http_reply(&mut s, "200 OK", "{}"),
    }
}

fn start(scenario: Scenario) -> (Arc<Fake>, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let fake = Arc::new(Fake { scenario, history: Default::default(), ws: Default::default(), requests: Default::default() });
    let f = fake.clone();
    std::thread::spawn(move || {
        for c in listener.incoming().flatten() {
            let f = f.clone();
            std::thread::spawn(move || handle(c, f));
        }
    });
    (fake, port)
}

struct Setup {
    comfy: Comfy,
    fake: Arc<Fake>,
    dir: PathBuf,
}

impl Setup {
    fn new(name: &str, scenario: Scenario) -> Self {
        let dir = std::env::temp_dir().join(format!("kage_test_comfy_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (fake, port) = start(scenario);
        let comfy = Comfy::new(&dir, Log::new(&dir.join("logs")));
        comfy.attach(port);
        Self { comfy, fake, dir }
    }

    fn run(&self, cancel: &AtomicBool) -> (Result<Vec<PathBuf>, RunError>, Vec<(u32, u32)>) {
        let mut progress = Vec::new();
        let r = self.comfy.run(&json!({"1": {"class_type": "Fake", "inputs": {}}}), "9", cancel, |v, m| progress.push((v, m)));
        (r, progress)
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn execution_success_before_the_history_entry_still_returns_the_image() {
    for pause_ms in [50, 0] {
        let s = Setup::new(&format!("success{pause_ms}"), Scenario::Success { pause_ms });
        let (result, progress) = s.run(&AtomicBool::new(false));
        let files = result.unwrap_or_else(|e| panic!("pause {pause_ms} ms: {e:?}"));
        assert_eq!(files, vec![s.comfy.output_dir().join("kage").join("x.png")]);
        assert_eq!(progress.last(), Some(&(5, 5)), "{progress:?}");
        assert!(progress.iter().all(|&(_, m)| m == 5), "progress of another prompt leaked in: {progress:?}");
        assert!(s.fake.requested("POST", "/interrupt").is_empty());
        let posted: Value = serde_json::from_str(&s.fake.requested("POST", "/prompt")[0]).unwrap();
        assert!(posted["client_id"].as_str().is_some_and(|c| c.len() == 32), "{posted}");
        assert!(posted["prompt"]["1"].is_object());
    }
}

#[test]
fn out_of_memory_error_is_classified() {
    for message in ["Allocation on device", "CUDA out of memory. Tried to allocate 2.00 GiB", "torch.OutOfMemoryError"] {
        let s = Setup::new("oom", Scenario::WsError(message));
        let (result, _) = s.run(&AtomicBool::new(false));
        match result {
            Err(RunError::Failed(Failure::OutOfMemory(m))) => {
                assert!(m.contains(message), "{m}");
                assert!(m.contains("KSampler"), "the node is named: {m}");
            }
            other => panic!("{message}: {other:?}"),
        }
    }
}

#[test]
fn other_execution_errors_are_not_out_of_memory() {
    let s = Setup::new("other_error", Scenario::WsError("Given groups=1, weight of size [64, 3, 3, 3]"));
    let (result, _) = s.run(&AtomicBool::new(false));
    match result {
        Err(RunError::Failed(Failure::Other(m))) => assert!(m.contains("Given groups=1"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn error_found_in_the_history_is_classified_too() {
    let s = Setup::new("history_oom", Scenario::HistoryError("CUDA out of memory"));
    match s.run(&AtomicBool::new(false)).0 {
        Err(RunError::Failed(Failure::OutOfMemory(m))) => assert_eq!(m, "CUDA out of memory"),
        other => panic!("{other:?}"),
    }
    let s = Setup::new("history_other", Scenario::HistoryError("bad"));
    match s.run(&AtomicBool::new(false)).0 {
        Err(RunError::Failed(Failure::Other(m))) => assert_eq!(m, "bad"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn restart_during_a_run_ends_it_quickly() {
    let s = Setup::new("restart", Scenario::Long);
    let comfy = s.comfy.clone();
    let bumper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        comfy.bump_generation();
    });
    let t0 = Instant::now();
    let (result, progress) = s.run(&AtomicBool::new(false));
    bumper.join().unwrap();
    assert!(matches!(result, Err(RunError::Failed(Failure::Restarted))), "{result:?}");
    assert!(t0.elapsed() < Duration::from_secs(3), "took {:?}", t0.elapsed());
    assert!(!progress.is_empty(), "the run was under way before the restart");
}

#[test]
fn cancel_interrupts_comfyui_and_returns_cancelled() {
    let s = Setup::new("cancel", Scenario::Long);
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c.store(true, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let (result, progress) = s.run(&cancel);
    setter.join().unwrap();
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(t0.elapsed() < Duration::from_secs(3), "took {:?}", t0.elapsed());
    assert!(!progress.is_empty());
    let interrupts = s.fake.requested("POST", "/interrupt");
    assert_eq!(interrupts.len(), 1, "{interrupts:?}");
    let body: Value = serde_json::from_str(&interrupts[0]).unwrap();
    assert_eq!(body["prompt_id"], ID);
    let queue = s.fake.requested("POST", "/queue");
    assert_eq!(queue.len(), 1);
    assert_eq!(serde_json::from_str::<Value>(&queue[0]).unwrap()["delete"], json!([ID]));
}

#[test]
fn cancel_set_before_the_run_does_not_hang() {
    let s = Setup::new("cancel_early", Scenario::Long);
    let t0 = Instant::now();
    let (result, _) = s.run(&AtomicBool::new(true));
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert_eq!(s.fake.requested("POST", "/interrupt").len(), 1);
}

#[test]
fn without_a_websocket_the_history_is_polled() {
    let s = Setup::new("no_ws", Scenario::NoWs { after_ms: 300 });
    let (result, progress) = s.run(&AtomicBool::new(false));
    let files = result.unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(files, vec![s.comfy.output_dir().join("kage").join("x.png")]);
    assert!(progress.is_empty());
    assert!(s.fake.requested("GET", "/history/p").len() >= 1);
}

#[test]
fn not_ready_is_reported_without_a_request() {
    let dir = std::env::temp_dir().join(format!("kage_test_comfy_notready_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let comfy = Comfy::new(&dir, Log::new(&dir.join("logs")));
    let r = comfy.run(&json!({}), "9", &AtomicBool::new(false), |_, _| {});
    assert!(matches!(r, Err(RunError::Failed(Failure::NotRunning(_)))), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
