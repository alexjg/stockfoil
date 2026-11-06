//! Wire interoperability vectors (design/spec.md, section 9).
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

fn only_item(g: &mut TestDocument) -> Vec<u8> {
    let plan = g.export(&ExportCursor::new());
    assert_eq!(plan.len(), 1);
    plan.item(0).map(|(_, bytes)| bytes).unwrap()
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
    let mut g = document_for(&k);
    let h = signed(&mut g, &k, vec![], b"hello");
    assert_eq!(
        hash_hex(&h),
        "ea2a999164d3363faffdc4263955bcb022710eb844362f87c46eccb5bf915436"
    );
    let item = only_item(&mut g);
    check("wire-v1-commit.hex", &item);
    let mut other = document();
    other.receive(item.as_slice()).unwrap();
    assert!(other.history().is_admitted(&h));
}

#[test]
fn fragment_vector() {
    let k = key(7);
    let mut g = document_for(&k);
    let mut parents = vec![];
    for _ in 0..100 {
        let p = g
            .commit(
                &b"shared long application key and repeated value"[..],
                Some(parents),
            )
            .unwrap();
        parents = vec![p.hash];
    }
    let head = g.commit(&b"checkpoint:129955"[..], Some(parents)).unwrap();
    assert_eq!(
        hash_hex(&head.hash),
        "0000bf36ab272ffdc22d98460eae2c0ab59764a0a5e28ac9ad9a7d791e5ae0ae"
    );
    // One signature on the depth-2 head certifies the whole chain.
    let outcome = g.attach_signature(&head, sign(&k, &head.hash)).unwrap();
    assert_eq!(outcome.admitted.len(), 101);
    let export = g.export(&ExportCursor::new());
    assert!(
        matches!(export.plan().id(0), Some(ItemId::Fragment(r)) if r.head == head.hash && r.boundary.is_empty())
    );
    let item = only_item(&mut g);
    check("wire-v1-fragment.hex", &item);
    let mut other = document();
    let Received::Admitted {
        items: receipts, ..
    } = other.receive(item.as_slice()).unwrap()
    else {
        panic!("expected immediate admission")
    };
    assert_eq!(receipts[0].members.len(), 101);
}
