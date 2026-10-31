//! Updates of the app itself from the GitHub releases: the newest release, downloading and checking its ZIP, and
//! putting the new exe in place.
//!
//! Windows does not let a running exe be overwritten, but it can be renamed. So the new exe takes its place while the
//! app still runs, and the next start removes the old one. Everything that can fail happens before the app closes.
//! Only the exe, README.md and LICENSE change; `models` and `data` stay as they are.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where to update by hand.
pub const RELEASES_PAGE: &str = "https://github.com/KRHa0024/AtozukeKageMOD/releases/latest";
const LATEST_API: &str = "https://api.github.com/repos/KRHa0024/AtozukeKageMOD/releases/latest";
/// A release's ZIP has to come from here.
const DOWNLOAD_PREFIX: &str = "https://github.com/KRHa0024/AtozukeKageMOD/releases/download/";
/// Replaces `LATEST_API` for tests, which then also accept a download from anywhere.
pub const URL_ENV: &str = "KAGE_UPDATE_URL";

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ARCHIVE: u64 = 256 << 20;
/// Folder of the ZIP in the release, and the files taken from it. The first one is required.
const ARCHIVE_TOP: &str = "AtozukeKageMOD";
const ARCHIVE_EXE: &str = "AtozukeKageMOD.exe";
const OTHER_FILES: [&str; 2] = ["README.md", "LICENSE"];
const WORK_DIR: &str = "update-tmp";
const ARCHIVE_NAME: &str = "update.zip";
/// The old process can take a moment to end after the new one started.
const CLEAN_TRIES: usize = 40;
const CLEAN_WAIT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Cancelled,
    Download(String),
    Verify(String),
    Install(String),
}

impl Error {
    pub fn detail(&self) -> &str {
        match self {
            Error::Cancelled => "cancelled",
            Error::Download(e) | Error::Verify(e) | Error::Install(e) => e,
        }
    }
}

fn parts(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('.').map(|p| p.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// Both must be plain x.y.z (the tags of this repository).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parts(candidate), parts(current)), (Some(c), Some(n)) if c > n)
}

/// The release in a GitHub API answer, when its ZIP is where it should be and comes with a digest.
pub fn parse_release(body: &str, url_prefix: &str) -> Option<Release> {
    let json: Value = serde_json::from_str(body).ok()?;
    let version = json["tag_name"].as_str()?;
    let name = format!("atozukekagemod-{version}.zip");
    let assets = json["assets"].as_array()?;
    let asset = assets.iter().find(|a| a["name"] == name.as_str())?;
    let url = asset["browser_download_url"].as_str().filter(|u| u.starts_with(url_prefix))?;
    let sha256 = asset["digest"].as_str()?.strip_prefix("sha256:")?;
    Some(Release { version: version.into(), url: url.into(), size: asset["size"].as_u64()?, sha256: sha256.to_ascii_lowercase() })
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = ureq::native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .user_agent(&format!("AtozukeKageMOD/{CURRENT}"))
        .timeout_connect(CHECK_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build())
}

/// The newest release, if it is newer than this app.
pub fn check() -> Result<Option<Release>, String> {
    let test_url = std::env::var(URL_ENV).ok();
    let url = test_url.as_deref().unwrap_or(LATEST_API);
    let response = agent()?.get(url).set("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
    let body = response.into_string().map_err(|e| e.to_string())?;
    let release = parse_release(&body, if test_url.is_some() { "" } else { DOWNLOAD_PREFIX }).ok_or("no usable release in the answer")?;
    Ok(is_newer(&release.version, CURRENT).then_some(release))
}

/// Next to the exe: renames only work inside one drive.
pub fn work_dir(exe: &Path) -> PathBuf {
    exe.with_file_name(WORK_DIR)
}

fn old_exe(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

fn save(release: &Release, path: &Path, progress: &AtomicU64, cancel: &AtomicBool) -> Result<(), Error> {
    if release.size > MAX_ARCHIVE {
        return Err(Error::Verify(format!("the archive is too large: {} bytes", release.size)));
    }
    let response = agent().map_err(Error::Download)?.get(&release.url).call().map_err(|e| Error::Download(e.to_string()))?;
    let mut reader = response.into_reader();
    let mut file = File::create(path).map_err(|e| Error::Install(e.to_string()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut total = 0;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut buffer).map_err(|e| Error::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > release.size {
            return Err(Error::Verify(format!("more data than the {} bytes announced", release.size)));
        }
        hash.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| Error::Install(e.to_string()))?;
        progress.store(total, Ordering::SeqCst);
    }
    if total != release.size {
        return Err(Error::Verify(format!("{total} of {} bytes", release.size)));
    }
    let digest = format!("{:x}", hash.finalize());
    if digest != release.sha256 {
        return Err(Error::Verify(format!("SHA-256 mismatch: {digest}")));
    }
    Ok(())
}

/// Downloads and checks the ZIP into `dir`. Nothing of it stays there on failure.
pub fn download(release: &Release, dir: &Path, progress: &AtomicU64, cancel: &AtomicBool) -> Result<PathBuf, Error> {
    fs::create_dir_all(dir).map_err(|e| Error::Install(e.to_string()))?;
    let path = dir.join(ARCHIVE_NAME);
    let result = save(release, &path, progress, cancel);
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result.map(|()| path)
}

fn unpack_file(archive: &mut zip::ZipArchive<File>, name: &str, dir: &Path) -> Result<bool, Error> {
    let mut entry = match archive.by_name(&format!("{ARCHIVE_TOP}/{name}")) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(false),
        Err(e) => return Err(Error::Verify(e.to_string())),
    };
    let mut out = File::create(dir.join(name)).map_err(|e| Error::Install(e.to_string()))?;
    std::io::copy(&mut entry, &mut out).map_err(|e| Error::Verify(e.to_string()))?;
    Ok(true)
}

/// Takes the files of the app out of the ZIP. The names are fixed, so a path inside the ZIP cannot lead elsewhere.
pub fn unpack(archive: &Path, dir: &Path) -> Result<(), Error> {
    let file = File::open(archive).map_err(|e| Error::Install(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::Verify(e.to_string()))?;
    if !unpack_file(&mut archive, ARCHIVE_EXE, dir)? {
        return Err(Error::Verify(format!("{ARCHIVE_EXE} is not in the archive")));
    }
    let mut head = [0; 2];
    File::open(dir.join(ARCHIVE_EXE)).and_then(|mut f| f.read_exact(&mut head)).map_err(|e| Error::Verify(e.to_string()))?;
    if &head != b"MZ" {
        return Err(Error::Verify(format!("{ARCHIVE_EXE} is not a program")));
    }
    for name in OTHER_FILES {
        unpack_file(&mut archive, name, dir)?;
    }
    Ok(())
}

/// Puts the unpacked files in place of `exe`, which may be running. A failure leaves `exe` as it was.
pub fn install(dir: &Path, exe: &Path) -> Result<(), Error> {
    let old = old_exe(exe);
    if old.exists() {
        fs::remove_file(&old).map_err(|e| Error::Install(format!("{}: {e}", old.display())))?;
    }
    fs::rename(exe, &old).map_err(|e| Error::Install(format!("{}: {e}", exe.display())))?;
    if let Err(e) = fs::rename(dir.join(ARCHIVE_EXE), exe) {
        let _ = fs::rename(&old, exe);
        return Err(Error::Install(format!("{}: {e}", exe.display())));
    }
    // Not worth undoing the update for.
    for name in OTHER_FILES {
        let _ = fs::rename(dir.join(name), exe.with_file_name(name));
    }
    Ok(())
}

/// Removes what an update left behind.
pub fn clean_up(exe: &Path) {
    let _ = fs::remove_dir_all(work_dir(exe));
    let old = old_exe(exe);
    for _ in 0..CLEAN_TRIES {
        if !old.exists() || fs::remove_file(&old).is_ok() {
            return;
        }
        std::thread::sleep(CLEAN_WAIT);
    }
}
