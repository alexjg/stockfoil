//! Fragment retirement and export through the native adapter.
mod support;
use ed25519_dalek::SigningKey;
use std::collections::BTreeSet;
use stockfoil::*;
use support::*;

fn at_level(k: &SigningKey, parents: &[CommitHash], level: usize, tag: &str) -> Vec<u8> {
    for i in 0u64.. {
        let payload = format!("{tag}-{i}").into_bytes();
        let hash = commit_hash(parents, &author(k), &payload);
        if hash.as_bytes().iter().take_while(|b| **b == 0).count() == level {
            return payload;
        }
    }
    unreachable!()
}

#[test]
fn received_fragments_retire_covered_standalone_items() {
    let alice = key(1);
    let mut a = document();
    let c0 = signed(&mut a, &alice, vec![], b"c0");
    let c1 = signed(&mut a, &alice, vec![c0], b"c1");
    let mut b = document();
    let cursor = sync(&mut a, &mut b, &ExportCursor::new());
    assert_eq!(cursor.len(), 2);
    // A fragment over both arrives later from elsewhere.
    let h = signed(&mut a, &alice, vec![c1], &at_level(&alice, &[c1], 1, "h"));
    let plan = a.export(&cursor);
    let retired: BTreeSet<_> = plan.plan().retired().iter().cloned().collect();
    assert_eq!(
        retired,
        BTreeSet::from([ItemId::Commit(c0), ItemId::Commit(c1)])
    );
    let cursor = sync(&mut a, &mut b, &cursor);
    assert_eq!(cursor.items().cloned().collect::<Vec<_>>().len(), 1);
    assert!(b.history().is_admitted(&h));
    assert_eq!(
        ExportCursor::from_bytes(&cursor.to_bytes()).unwrap(),
        cursor
    );
}

#[test]
fn fragments_inside_admitted_fragments_are_not_added() {
    let alice = key(1);
    let mut a = document();
    let c0 = signed(&mut a, &alice, vec![], b"c0");
    let h1 = signed(&mut a, &alice, vec![c0], &at_level(&alice, &[c0], 1, "h1"));
    // `c` keeps the depth-1 fragment {c0, h1}; `a` goes on to a depth-2
    // fragment that covers it.
    let mut c = document();
    sync(&mut a, &mut c, &ExportCursor::new());
    let x = signed(&mut a, &alice, vec![h1], b"x");
    let h2 = signed(&mut a, &alice, vec![x], &at_level(&alice, &[x], 2, "h2"));
    let mut b = document();
    sync(&mut a, &mut b, &ExportCursor::new());
    // The smaller fragment arriving afterwards adds nothing to export.
    sync(&mut c, &mut b, &ExportCursor::new());
    let plan = b.export(&ExportCursor::new());
    assert_eq!(plan.len(), 1);
    let item = b.history().export_item(plan.plan(), 0).unwrap();
    assert!(matches!(&item.meta.id, ItemId::Fragment(range) if range.head == h2));
}
