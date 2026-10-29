//! In-memory log shown in Settings > Log, mirrored to `<data>/logs/kage.log`. Log lines are English.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Clone)]
pub struct Line {
    pub time: String,
    pub level: Level,
    pub text: String,
}

const MAX_LINES: usize = 5000;
const MAX_FILE_BYTES: u64 = 5 << 20;

struct Inner {
    lines: Vec<Line>,
    file: Option<File>,
    /// Bumped on every line so the UI can tell when to repaint / scroll.
    serial: u64,
}

#[derive(Clone)]
pub struct Log {
    inner: Arc<Mutex<Inner>>,
    dir: PathBuf,
}

impl Log {
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("kage.log");
        if std::fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&path, dir.join("kage.1.log"));
        }
        let file = OpenOptions::new().create(true).append(true).open(&path).ok();
        Self { inner: Arc::new(Mutex::new(Inner { lines: Vec::new(), file, serial: 0 })), dir: dir.to_path_buf() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn add(&self, level: Level, text: impl Into<String>) {
        let text = text.into();
        let (date, time) = now_local();
        let mut g = self.inner.lock().unwrap();
        if let Some(f) = &mut g.file {
            let _ = writeln!(f, "{date} {time}  {:<5}  {text}", level.tag());
        }
        g.lines.push(Line { time, level, text });
        if g.lines.len() > MAX_LINES {
            let drop = g.lines.len() - MAX_LINES;
            g.lines.drain(..drop);
        }
        g.serial += 1;
    }

    pub fn info(&self, text: impl Into<String>) {
        self.add(Level::Info, text)
    }

    pub fn warn(&self, text: impl Into<String>) {
        self.add(Level::Warn, text)
    }

    pub fn error(&self, text: impl Into<String>) {
        self.add(Level::Error, text)
    }

    pub fn snapshot(&self) -> (u64, Vec<Line>) {
        let g = self.inner.lock().unwrap();
        (g.serial, g.lines.clone())
    }

    pub fn serial(&self) -> u64 {
        self.inner.lock().unwrap().serial
    }
}

/// Local date and time ("2026-10-30", "12:40:02") without a time-zone crate.
pub fn now_local() -> (String, String) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut t) };
        (
            format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
            format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond),
        )
    }
    #[cfg(not(windows))]
    {
        let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        ("1970-01-01".into(), format!("{:02}:{:02}:{:02}", s / 3600 % 24, s / 60 % 60, s % 60))
    }
}

/// Compact local timestamp for file names ("20261030-124002").
pub fn stamp() -> String {
    let (d, t) = now_local();
    format!("{}-{}", d.replace('-', ""), t.replace(':', ""))
}
