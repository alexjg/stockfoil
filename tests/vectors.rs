//! Wire v2 interoperability vectors (design/wire-v2.md, section 9). The commit
//! identities are the same as wire v1's vectors: only the envelope changed.
//! Regenerate with `STOCKFOIL_UPDATE_VECTORS=1 cargo test --test vectors`.
mod support;
use stockfoil::*;
use support::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn check(name: &str, bytes: &[u8]) {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let encoded = hex(bytes) + "\n";
    if std::env::var_os("STOCKFOIL_UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &encoded).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        encoded,
        "{name} changed"
    );
}

fn only_item(g: &mut TestGraph) -> Vec<u8> {
    let plan = g.export(&ExportCursor::new());
    assert_eq!(plan.len(), 1);
    let mut bytes = vec![];
    g.write_item(&plan, 0, &mut bytes).unwrap();
    bytes
}

fn hash_hex(h: &CommitHash) -> String {
    hex(h.as_bytes())
}

#[test]
fn standalone_commit_vector() {
    let k = key(7);
    assert_eq!(
        hex(author(&k).as_slice()),
        "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
    );
    let mut g = graph();
    let h = signed(&mut g, &k, vec![], b"hello");
    assert_eq!(
        hash_hex(&h),
        "ea2a999164d3363faffdc4263955bcb022710eb844362f87c46eccb5bf915436"
    );
    let item = only_item(&mut g);
    check("wire-v2-commit.hex", &item);
    let mut other = graph();
    other.receive(item.as_slice()).unwrap();
    assert!(other.history().is_admitted(&h));
}

#[test]
fn fragment_vector() {
    let k = key(7);
    let mut g = graph();
    let mut parents = vec![];
    for _ in 0..100 {
        let p = g
            .commit(
                author(&k),
                parents,
                &b"shared long application key and repeated value"[..],
            )
            .unwrap();
        parents = vec![p.hash];
    }
    let head = g
        .commit(author(&k), parents, &b"checkpoint:129955"[..])
        .unwrap();
    assert_eq!(
        hash_hex(&head.hash),
        "0000bf36ab272ffdc22d98460eae2c0ab59764a0a5e28ac9ad9a7d791e5ae0ae"
    );
    // One signature on the depth-2 head certifies the whole chain.
    let admitted = g.sign(author(&k), head.hash, sign(&k, &head.hash)).unwrap();
    assert!(
        matches!(&admitted[..], [ItemId::Fragment(r)] if r.head == head.hash && r.boundary.is_empty())
    );
    let item = only_item(&mut g);
    check("wire-v2-fragment.hex", &item);
    let mut other = graph();
    let receipt = other.receive(item.as_slice()).unwrap();
    assert_eq!(receipt.members.len(), 101);
}
