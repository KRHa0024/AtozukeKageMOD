//! The ComfyUI graphs and parameters the shading was tuned with. tests/fixtures/graphs.json is a snapshot of
//! `shade_graph("IN.png", "PROMPT", 1024, 42)` and `mask_graph("IN.png")`; a change to either is deliberate or a bug.
//! Regenerate with `KAGE_UPDATE_GRAPHS=1 cargo test --release --test graphs` after changing them on purpose.

use std::path::PathBuf;

use kage::models::MODELS;
use kage::pipeline::{mask_graph, shade_graph, Params};
use serde_json::{json, Value};

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graphs.json")
}

fn current() -> Value {
    json!({
        "shade": shade_graph("IN.png", "PROMPT", 1024, 42),
        "mask": mask_graph("IN.png"),
    })
}

fn params(light_dir: usize) -> Params {
    Params { light_dir, extra: String::new(), strength: 1.0, background: false, gray: false, ghost: false, seed: 0 }
}

#[test]
fn graphs_match_the_snapshot() {
    let now = current();
    if std::env::var_os("KAGE_UPDATE_GRAPHS").is_some() {
        std::fs::write(snapshot_path(), serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
    }
    let text = std::fs::read_to_string(snapshot_path()).unwrap_or_else(|e| panic!("{}: {e}", snapshot_path().display()));
    let expected: Value = serde_json::from_str(&text).unwrap();
    assert!(
        now == expected,
        "the graphs changed.\n--- snapshot ---\n{}\n--- now ---\n{}",
        serde_json::to_string_pretty(&expected).unwrap(),
        serde_json::to_string_pretty(&now).unwrap()
    );
}

#[test]
fn tuned_sampler_values() {
    let g = shade_graph("IN.png", "PROMPT", 1024, 42);
    let k = &g["7"];
    assert_eq!(k["class_type"], "KSampler");
    let i = &k["inputs"];
    assert_eq!(i["steps"].as_u64(), Some(25));
    assert_eq!(i["cfg"].as_f64(), Some(1.0));
    assert_eq!(i["sampler_name"], "euler");
    assert_eq!(i["scheduler"], "simple");
    assert_eq!(i["denoise"].as_f64(), Some(1.0));
    assert_eq!(i["seed"].as_u64(), Some(42));

    let enc = &g["5"]["inputs"];
    assert_eq!(g["5"]["class_type"], "TextEncodeQwenImage21");
    assert_eq!(enc["resolution"].as_u64(), Some(1024));
    assert_eq!(enc["prompt"], "PROMPT");
    assert_eq!(enc["negative_prompt"], "");
    assert_eq!(g["20"]["inputs"]["image"], "IN.png");
    assert_eq!(shade_graph("IN.png", "PROMPT", 1536, 1)["5"]["inputs"]["resolution"].as_u64(), Some(1536));

    // The files come from the verified model table.
    assert_eq!(g["1"]["inputs"]["unet_name"], MODELS[0].file);
    assert_eq!(g["2"]["inputs"]["clip_name"], MODELS[1].file);
    assert_eq!(g["3"]["inputs"]["vae_name"], MODELS[2].file);
    assert_eq!(g["9"]["class_type"], "SaveImage");
}

#[test]
fn mask_graph_uses_birefnet_with_inverted_mask_as_alpha() {
    let g = mask_graph("IN.png");
    assert_eq!(g["1"]["inputs"]["image"], "IN.png");
    assert_eq!(g["2"]["inputs"]["bg_removal_name"], MODELS[3].file);
    assert_eq!(g["3"]["class_type"], "RemoveBackground");
    assert_eq!(g["4"]["class_type"], "InvertMask");
    assert_eq!(g["5"]["class_type"], "JoinImageWithAlpha");
    assert_eq!(g["9"]["class_type"], "SaveImage");
}

#[test]
fn only_core_nodes_are_used() {
    // ComfyUI runs with --disable-all-custom-nodes: every class must be one of these.
    let core = [
        "UNETLoader", "CLIPLoader", "VAELoader", "QwenImage21Cache", "LoadImage", "TextEncodeQwenImage21", "KSampler", "VAEDecode",
        "SaveImage", "LoadBackgroundRemovalModel", "RemoveBackground", "InvertMask", "JoinImageWithAlpha",
    ];
    for g in [shade_graph("IN.png", "P", 1024, 1), mask_graph("IN.png")] {
        for (id, node) in g.as_object().unwrap() {
            let class = node["class_type"].as_str().unwrap();
            assert!(core.contains(&class), "node {id}: {class}");
        }
    }
}

#[test]
fn prompt_mentions_the_light_direction() {
    let p = params(0).prompt();
    assert!(p.contains("light coming from the upper left"), "{p}");
    assert!(p.contains(", lit by light coming from the upper left:"), "{p}");
    assert!(!p.contains("{light}"));
    // Automatic light: the prompt says nothing about a direction.
    let auto = params(4).prompt();
    assert!(!auto.contains("lit by") && !auto.contains("light coming from") && !auto.contains("{light}"), "{auto}");
    // Every direction has its own words.
    let all: std::collections::HashSet<String> = (0..9).map(|i| params(i).prompt()).collect();
    assert_eq!(all.len(), 9);
    // An extra instruction is added, with or without a direction.
    let extra = Params { extra: " warm sunset ".into(), ..params(1) }.prompt();
    assert!(extra.contains("warm sunset coming from above"), "{extra}");
    let extra_auto = Params { extra: "warm sunset".into(), ..params(4) }.prompt();
    assert!(extra_auto.contains(", lit by warm sunset:"), "{extra_auto}");
}
