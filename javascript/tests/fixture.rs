//! Writes `test/fixtures/rust-items.json`: items exported by the native Rust
//! `Document` with the core test codec, for the JavaScript interop test.
//!
//! The content is deterministic. The test fails if the checked-in fixture is
//! stale; regenerate it with `STOCKFOIL_UPDATE_FIXTURES=1 cargo test -p stockfoil-wasm`.
#[path = "../../stockfoil/tests/support/mod.rs"]
mod support;

use std::fmt::Write as _;
use stockfoil::{ExportCursor, ItemId};
use support::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn fixture() -> String {
    let alice = key(1);
    let mut a = document();
    let mut commits = Vec::new();
    let mut tip = signed(&mut a, &alice, vec![], b"root");
    commits.push((tip, b"root".to_vec()));
    for i in 0..3 {
        let payload = format!("member {i}").repeat(50).into_bytes();
        tip = signed(&mut a, &alice, vec![tip], &payload);
        commits.push((tip, payload));
    }
    let payload = payload_at_depth(&alice, &[tip], 1, "head");
    tip = signed(&mut a, &alice, vec![tip], &payload);
    commits.push((tip, payload));
    tip = signed(&mut a, &alice, vec![tip], b"after");
    commits.push((tip, b"after".to_vec()));

    let plan = a.export(&ExportCursor::new());
    let mut items = Vec::new();
    for i in 0..plan.len() {
        let bytes = plan.item(i).map(|(_, bytes)| bytes).unwrap();
        let kind = match plan.plan().id(i).unwrap() {
            ItemId::Commit(_) => "commit",
            ItemId::Fragment(_) => "fragment",
        };
        items.push(format!(
            "    {{ \"kind\": \"{kind}\", \"head\": \"{}\", \"bytes\": \"{}\" }}",
            hex(plan.plan().id(i).unwrap().head().as_slice()),
            hex(&bytes)
        ));
    }
    let commits: Vec<_> = commits
        .iter()
        .map(|(hash, payload)| {
            format!(
                "    {{ \"hash\": \"{}\", \"payload\": \"{}\" }}",
                hex(hash.as_slice()),
                hex(payload)
            )
        })
        .collect();
    format!(
        "{{\n  \"format\": \"{}\",\n  \"author\": \"{}\",\n  \"items\": [\n{}\n  ],\n  \"commits\": [\n{}\n  ]\n}}\n",
        format().as_str(),
        hex(author(&alice).as_slice()),
        items.join(",\n"),
        commits.join(",\n"),
    )
}

#[test]
fn rust_items_fixture_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test/fixtures/rust-items.json");
    let expected = fixture();
    let update = std::env::var_os("STOCKFOIL_UPDATE_FIXTURES").is_some();
    match std::fs::read_to_string(path) {
        Ok(current) if !update => assert!(current == expected, "stale fixture: {path}"),
        _ => std::fs::write(path, expected).unwrap(),
    }
}
