//! Write seed corpora for the byte-oriented targets:
//! `cargo +nightly run --example gen_seeds` (from `fuzz/`).
use std::fs;
use std::path::Path;
use stockfoil::*;
use stockfoil_fuzz::templates::templates;

fn write(dir: &str, name: String, bytes: &[u8]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus").join(dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), bytes).unwrap();
}

fn main() {
    let t = templates();
    for (i, item) in t.items.iter().enumerate() {
        let stage = t.stages.iter().position(|s| s.contains(&i)).unwrap();
        let n = header_len(&item.bytes, &Limits::default()).unwrap().unwrap();
        // wire_header: decode mode with roomy limits, header alone and whole item.
        let mut seed = vec![0, 8, 240, 240, 240, 240];
        seed.extend(&item.bytes[..n]);
        write("wire_header", format!("header-{i}"), &seed);
        let mut seed = vec![0, 9, 7, 7, 5, 5];
        seed.extend(&item.bytes);
        write("wire_header", format!("item-{i}"), &seed);
        // admission_bytes: raw mode, several host shapes.
        for (j, shape) in [[15u8, 15, 8, 6, 6], [0, 0, 0, 0, 0], [5, 3, 1, 2, 3], [14, 1, 4, 4, 1]].iter().enumerate() {
            let mut seed = vec![0, shape[0], shape[1], shape[2], shape[3], shape[4], 0, stage as u8, 0];
            seed.extend(&item.bytes);
            write("admission_bytes", format!("item-{i}-{j}"), &seed);
        }
    }
    // Crafted-mode seeds: all-zero choices give a signed, valid commit.
    for s in 0..=t.stages.len() as u8 {
        let mut seed = vec![3, 15, 15, 8, 6, 6, 0, s, 0];
        seed.extend([0u8; 64]);
        write("admission_bytes", format!("crafted-{s}"), &seed);
    }
    for d in ["admission_replies", "history_ops"] {
        write(d, "empty".into(), &[0; 64]);
    }
    println!("{} template items", t.items.len());
}
