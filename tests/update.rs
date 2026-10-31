//! The app's own update, against a small HTTP server on 127.0.0.1: finding a release, downloading and checking its
//! ZIP, and putting the files in place without touching anything else.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kage::update::{check, clean_up, download, install, is_newer, parse_release, unpack, work_dir, Error, Release, URL_ENV};
use serde_json::json;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const PREFIX: &str = "https://github.com/KRHa0024/AtozukeKageMOD/releases/download/";

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("update").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Answers `times` requests with `body`, cut off after `send` bytes when that is shorter.
fn serve(body: Vec<u8>, send: usize, times: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(times) {
            let mut stream = stream.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body[..send.min(body.len())]);
        }
    });
    format!("http://127.0.0.1:{port}/file")
}

fn release_of(body: &[u8], url: String) -> Release {
    Release { version: "9.9.9".into(), url, size: body.len() as u64, sha256: format!("{:x}", Sha256::digest(body)) }
}

/// The layout of a release ZIP, with a different folder or without the exe when a test wants that.
fn archive(top: &str, exe: Option<&[u8]>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    if let Some(exe) = exe {
        zip.start_file(format!("{top}/AtozukeKageMOD.exe"), options).unwrap();
        zip.write_all(exe).unwrap();
    }
    for (name, text) in [("README.md", "new readme"), ("LICENSE", "new license"), ("models/vae/note.txt", "must stay out")] {
        zip.start_file(format!("{top}/{name}"), options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn answer(version: &str, assets: serde_json::Value) -> String {
    json!({"tag_name": version, "assets": assets}).to_string()
}

fn asset(name: &str, url: &str, digest: Option<&str>) -> serde_json::Value {
    json!({"name": name, "browser_download_url": url, "size": 123, "digest": digest})
}

#[test]
fn versions_compare_as_numbers() {
    assert!(is_newer("0.1.5", "0.1.4"));
    assert!(is_newer("0.1.10", "0.1.9"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.1.4", "0.1.4"));
    assert!(!is_newer("0.1.3", "0.1.4"));
}

#[test]
fn anything_but_plain_versions_is_never_newer() {
    for odd in ["v9.0.0", "9.0", "9.0.0.0", "9.0.0-beta", "", "latest"] {
        assert!(!is_newer(odd, "0.1.4"), "{odd}");
        assert!(!is_newer("9.0.0", odd), "{odd}");
    }
}

#[test]
fn a_release_is_read_from_the_api_answer() {
    let url = format!("{PREFIX}0.1.5/atozukekagemod-0.1.5.zip");
    let other = asset("notes.txt", "https://example.com/notes.txt", None);
    let body = answer("0.1.5", json!([other, asset("atozukekagemod-0.1.5.zip", &url, Some("sha256:ABC123"))]));
    assert_eq!(parse_release(&body, PREFIX), Some(Release { version: "0.1.5".into(), url, size: 123, sha256: "abc123".into() }));
}

#[test]
fn a_release_without_a_trustworthy_zip_is_ignored() {
    let name = "atozukekagemod-0.1.5.zip";
    let good = format!("{PREFIX}0.1.5/{name}");
    assert_eq!(parse_release(&answer("0.1.5", json!([asset(name, "https://example.com/a.zip", Some("sha256:ab"))])), PREFIX), None, "other host");
    assert_eq!(parse_release(&answer("0.1.5", json!([asset(name, &good, None)])), PREFIX), None, "no digest");
    assert_eq!(parse_release(&answer("0.1.5", json!([asset(name, &good, Some("md5:ab"))])), PREFIX), None, "other digest");
    assert_eq!(parse_release(&answer("0.1.5", json!([asset("other.zip", &good, Some("sha256:ab"))])), PREFIX), None, "other asset");
    assert_eq!(parse_release(&answer("0.1.5", json!([])), PREFIX), None, "no assets");
    assert_eq!(parse_release("not json", PREFIX), None);
}

#[test]
fn the_check_says_whether_the_release_is_newer() {
    let answer_for = |version: &str| {
        let zip = format!("http://127.0.0.1/atozukekagemod-{version}.zip");
        answer(version, json!([asset(&format!("atozukekagemod-{version}.zip"), &zip, Some("sha256:ab"))]))
    };
    // The only test that sets the variable.
    std::env::set_var(URL_ENV, serve(answer_for("99.0.0").into_bytes(), usize::MAX, 1));
    assert_eq!(check().unwrap().map(|r| r.version), Some("99.0.0".into()));
    std::env::set_var(URL_ENV, serve(answer_for("0.0.1").into_bytes(), usize::MAX, 1));
    assert_eq!(check().unwrap(), None);
    std::env::set_var(URL_ENV, serve(b"<html>".to_vec(), usize::MAX, 1));
    assert!(check().is_err());
    std::env::remove_var(URL_ENV);
}

#[test]
fn a_download_is_kept_only_when_it_matches() {
    let body = vec![7; 200_000];
    let dir = scratch("download");
    let (progress, cancel) = (AtomicU64::new(0), AtomicBool::new(false));

    let good = release_of(&body, serve(body.clone(), usize::MAX, 1));
    let path = download(&good, &dir, &progress, &cancel).unwrap();
    assert_eq!(fs::read(&path).unwrap(), body);
    assert_eq!(progress.load(Ordering::SeqCst), body.len() as u64);
    fs::remove_file(&path).unwrap();

    let mut wrong = release_of(&body, serve(body.clone(), usize::MAX, 1));
    wrong.sha256 = "0".repeat(64);
    assert!(matches!(download(&wrong, &dir, &progress, &cancel), Err(Error::Verify(_))));
    assert!(!path.exists());

    let cut = release_of(&body, serve(body.clone(), 1000, 1));
    assert!(download(&cut, &dir, &progress, &cancel).is_err());
    assert!(!path.exists());

    let mut small = release_of(&body, serve(body.clone(), usize::MAX, 1));
    small.size = 1000;
    assert!(matches!(download(&small, &dir, &progress, &cancel), Err(Error::Verify(_))));
    assert!(!path.exists());

    let mut huge = release_of(&body, String::new());
    huge.size = u64::MAX;
    assert!(matches!(download(&huge, &dir, &progress, &cancel), Err(Error::Verify(_))));
}

#[test]
fn a_cancelled_download_leaves_nothing() {
    let body = vec![7; 200_000];
    let dir = scratch("cancel");
    let release = release_of(&body, serve(body.clone(), usize::MAX, 1));
    assert_eq!(download(&release, &dir, &AtomicU64::new(0), &AtomicBool::new(true)), Err(Error::Cancelled));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn a_server_that_cannot_be_reached_is_a_download_failure() {
    let dir = scratch("offline");
    let release = release_of(b"x", "http://127.0.0.1:9/file".into());
    assert!(matches!(download(&release, &dir, &AtomicU64::new(0), &AtomicBool::new(false)), Err(Error::Download(_))));
}

#[test]
fn only_the_files_of_the_app_are_taken_from_the_zip() {
    let dir = scratch("unpack");
    let zip = dir.join("update.zip");
    fs::write(&zip, archive("AtozukeKageMOD", Some(b"MZ new exe"))).unwrap();
    unpack(&zip, &dir).unwrap();
    assert_eq!(fs::read(dir.join("AtozukeKageMOD.exe")).unwrap(), b"MZ new exe");
    assert_eq!(fs::read_to_string(dir.join("README.md")).unwrap(), "new readme");
    assert_eq!(fs::read_to_string(dir.join("LICENSE")).unwrap(), "new license");
    assert!(!dir.join("models").exists());
}

#[test]
fn a_zip_without_a_usable_exe_is_refused() {
    let dir = scratch("bad_zip");
    let zip = dir.join("update.zip");
    for (name, bytes) in [
        ("no exe", archive("AtozukeKageMOD", None)),
        ("other folder", archive("Other", Some(b"MZ"))),
        ("not a program", archive("AtozukeKageMOD", Some(b"hello"))),
        ("empty exe", archive("AtozukeKageMOD", Some(b""))),
        ("not a zip", b"hello".to_vec()),
    ] {
        fs::write(&zip, bytes).unwrap();
        assert!(matches!(unpack(&zip, &dir), Err(Error::Verify(_))), "{name}");
    }
}

/// An installed app with its models and data.
fn installed(name: &str) -> (PathBuf, PathBuf) {
    let root = scratch(name);
    let exe = root.join("AtozukeKageMOD.exe");
    fs::write(&exe, "old exe").unwrap();
    fs::write(root.join("README.md"), "old readme").unwrap();
    fs::create_dir_all(root.join("models/vae")).unwrap();
    fs::write(root.join("models/vae/model.safetensors"), "model").unwrap();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/settings.json"), "settings").unwrap();
    (root, exe)
}

fn unpacked(exe: &Path) -> PathBuf {
    let dir = work_dir(exe);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("AtozukeKageMOD.exe"), "new exe").unwrap();
    fs::write(dir.join("README.md"), "new readme").unwrap();
    fs::write(dir.join("LICENSE"), "new license").unwrap();
    dir
}

#[test]
fn an_update_replaces_the_app_and_nothing_else() {
    let (root, exe) = installed("install");
    let dir = unpacked(&exe);
    install(&dir, &exe).unwrap();
    assert_eq!(fs::read_to_string(&exe).unwrap(), "new exe");
    assert_eq!(fs::read_to_string(root.join("AtozukeKageMOD.exe.old")).unwrap(), "old exe");
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), "new readme");
    assert_eq!(fs::read_to_string(root.join("LICENSE")).unwrap(), "new license");
    assert_eq!(fs::read_to_string(root.join("models/vae/model.safetensors")).unwrap(), "model");
    assert_eq!(fs::read_to_string(root.join("data/settings.json")).unwrap(), "settings");
    clean_up(&exe);
    assert!(!root.join("AtozukeKageMOD.exe.old").exists());
    assert!(!dir.exists());
    assert_eq!(fs::read_to_string(&exe).unwrap(), "new exe");
}

#[test]
fn an_update_that_cannot_finish_leaves_the_app_as_it_was() {
    let (root, exe) = installed("install_failure");
    let dir = work_dir(&exe);
    fs::create_dir_all(&dir).unwrap();
    assert!(matches!(install(&dir, &exe), Err(Error::Install(_))));
    assert_eq!(fs::read_to_string(&exe).unwrap(), "old exe");
    assert!(!root.join("AtozukeKageMOD.exe.old").exists());
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), "old readme");
}

#[test]
fn a_leftover_of_an_earlier_update_does_not_stop_the_next_one() {
    let (root, exe) = installed("leftover");
    fs::write(root.join("AtozukeKageMOD.exe.old"), "older exe").unwrap();
    let dir = unpacked(&exe);
    install(&dir, &exe).unwrap();
    assert_eq!(fs::read_to_string(&exe).unwrap(), "new exe");
    assert_eq!(fs::read_to_string(root.join("AtozukeKageMOD.exe.old")).unwrap(), "old exe");
}

#[test]
fn clean_up_has_nothing_to_do_after_a_normal_start() {
    let (root, exe) = installed("clean");
    clean_up(&exe);
    assert_eq!(fs::read_to_string(&exe).unwrap(), "old exe");
    assert!(root.join("models/vae/model.safetensors").exists());
}
