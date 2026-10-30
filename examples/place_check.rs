//! Checks "Choose file" placement: link on the same drive, copy across drives, nothing written on a mismatch.
//!
//!     cargo run --release --example place_check -- <official birefnet.safetensors> <other file> <same-drive dir> <other-drive dir> <scratch>

use std::path::PathBuf;
use std::time::Duration;

use kage::log::Log;
use kage::models::{model_path, Verifier, MODELS};

fn wait(v: &Verifier) {
    std::thread::sleep(Duration::from_millis(100));
    while v.running() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    let a: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let (official, other, same_drive, other_drive, scratch) = (&a[0], &a[1], &a[2], &a[3], &a[4]);
    let log = Log::new(&scratch.join("logs"));
    let v = Verifier::new(scratch, log.clone());
    let i = 3; // birefnet
    for (what, dir) in [("same drive", same_drive), ("other drive", other_drive)] {
        v.place(i, official.clone(), dir.clone());
        wait(&v);
        let dest = model_path(dir, &MODELS[i]);
        println!("{what}: status {:?} note {:?} placed {}", v.statuses()[i], v.notes()[i], dest.is_file());
    }
    let fake = scratch.join("fake.safetensors");
    std::fs::File::create(&fake).unwrap().set_len(MODELS[i].size).unwrap();
    let empty = scratch.join("empty_models");
    for (what, src) in [("wrong size", other), ("same size, other content", &fake)] {
        v.place(i, src.clone(), empty.clone());
        wait(&v);
        println!("{what}: status {:?} note {:?} wrote anything: {}", v.statuses()[i], v.notes()[i], empty.exists());
    }
    let (_, lines) = log.snapshot();
    for l in lines {
        println!("  {} {}", l.level.tag(), l.text);
    }
}
