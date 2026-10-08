//! End-to-end behavior through the native adapter.
mod support;
use std::collections::HashMap;
use stockfoil::*;
use support::*;

#[test]
fn commit_sign_export_receive_round_trip() {
    let alice = key(1);
    let mut a = graph();
    let first = signed(&mut a, &alice, vec![], b"hello");
    let second = signed(&mut a, &alice, vec![first], b"world");
    assert!(a.history().is_admitted(&second));
    assert_eq!(
        a.history().heads(Heads::Admitted).collect::<Vec<_>>(),
        vec![second]
    );

    let mut b = graph();
    let cursor = sync(&mut a, &mut b, &ExportCursor::new());
    assert!(b.history().is_admitted(&first));
    assert!(b.history().is_admitted(&second));
    assert!(b.history().is_ancestor(&first, &second));
    assert_eq!(b.payload(&second).unwrap(), b"world");

    // Nothing new to send.
    assert!(a.export(&cursor).is_empty());
}

#[test]
fn commit_hash_matches_independent_rule() {
    let alice = key(1);
    let mut g = graph();
    let p = g.commit(author(&alice), vec![], &b"x"[..]).unwrap();
    assert_eq!(p.hash, commit_hash(&[], &author(&alice), b"x"));
    let q = g
        .commit(author(&alice), vec![p.hash, p.hash], &b"y"[..])
        .unwrap();
    assert_eq!(
        q.hash,
        commit_hash(&[p.hash, p.hash], &author(&alice), b"y")
    );
}

#[test]
fn drafts_are_delivered_but_not_admitted_until_signed() {
    let alice = key(1);
    let mut g = graph();
    let p = g.commit(author(&alice), vec![], &b"draft"[..]).unwrap();
    assert!(!g.history().is_admitted(&p.hash));
    assert_eq!(
        g.history().heads(Heads::Delivered).collect::<Vec<_>>(),
        vec![p.hash]
    );
    assert_eq!(g.history().heads(Heads::Admitted).count(), 0);
    assert!(g.export(&ExportCursor::new()).is_empty());

    // Signing a child first admits nothing: its parent is still a draft.
    let c = g
        .commit(author(&alice), vec![p.hash], &b"child"[..])
        .unwrap();
    assert_eq!(
        g.sign(author(&alice), c.hash, sign(&alice, &c.hash))
            .unwrap(),
        vec![]
    );
    // Signing the parent admits both, in order.
    let admitted = g
        .sign(author(&alice), p.hash, sign(&alice, &p.hash))
        .unwrap();
    assert_eq!(
        admitted,
        vec![ItemId::Commit(p.hash), ItemId::Commit(c.hash)]
    );
    assert!(g.history().is_admitted(&c.hash));
    assert_eq!(g.history().drafts().count(), 0);
}

#[test]
fn wrong_signer_or_bad_signature_is_rejected() {
    let (alice, bob) = (key(1), key(2));
    let mut g = graph();
    let p = g.commit(author(&alice), vec![], &b"x"[..]).unwrap();
    assert!(matches!(
        g.sign(author(&alice), p.hash, sign(&bob, &p.hash)),
        Err(Error::InvalidSignature { .. })
    ));
    // Bob may co-sign, but only Alice's signature covers Alice's commit.
    assert_eq!(
        g.sign(author(&bob), p.hash, sign(&bob, &p.hash)).unwrap(),
        vec![]
    );
    assert!(!g.history().is_admitted(&p.hash));
    g.sign(author(&alice), p.hash, sign(&alice, &p.hash))
        .unwrap();
    assert!(matches!(
        g.sign(author(&alice), p.hash, sign(&alice, &p.hash)),
        Err(Error::NotDraft(_))
    ));
}

#[test]
fn missing_parents_are_rejected_before_reading_payload() {
    let alice = key(1);
    let mut g = graph();
    let unknown = CommitHash::from([7; 32]);
    struct Panics;
    impl std::io::Read for Panics {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("payload read before parent validation")
        }
    }
    assert_eq!(
        g.commit(author(&alice), vec![unknown], Panics).unwrap_err(),
        Error::MissingParent(unknown)
    );
}

#[test]
fn local_fragment_certificates_cover_their_members() {
    let alice = key(1);
    let mut a = graph();
    let base = signed(&mut a, &alice, vec![], b"base");
    let mut tip = base;
    let mut chain = vec![];
    for i in 0..5 {
        tip = signed(&mut a, &alice, vec![tip], format!("m{i}").as_bytes());
        chain.push(tip);
    }
    let payload = payload_at_depth(&alice, &[tip], 1, "checkpoint");
    let head = signed(&mut a, &alice, vec![tip], &payload);
    // The depth-1 head assembles a fragment covering the whole chain, so the
    // standalone certificates it covers leave the inventory.
    let plan = a.export(&ExportCursor::new());
    let ids: Vec<_> = (0..plan.len())
        .map(|i| plan.id(i).unwrap().clone())
        .collect();
    assert_eq!(ids.len(), 1, "{ids:?}");
    let ItemId::Fragment(range) = &ids[0] else {
        panic!("expected a fragment")
    };
    assert_eq!(range.head, head);
    assert!(range.boundary.is_empty());

    let mut b = graph();
    sync(&mut a, &mut b, &ExportCursor::new());
    for h in chain.iter().chain([&base, &head]) {
        assert!(b.history().is_admitted(h));
    }
    assert_eq!(b.payload(&chain[2]).unwrap(), b"m2");
}

#[test]
fn received_items_wait_for_dependencies() {
    let alice = key(1);
    let mut a = graph();
    let first = signed(&mut a, &alice, vec![], b"1");
    let second = signed(&mut a, &alice, vec![first], b"2");
    let plan = a.export(&ExportCursor::new());
    let mut items = vec![];
    for i in 0..plan.len() {
        let mut bytes = vec![];
        a.write_item(&plan, i, &mut bytes).unwrap();
        items.push((plan.id(i).unwrap().clone(), bytes));
    }
    let (_, second_item) = items.iter().find(|(id, _)| id.head() == second).unwrap();
    let (_, first_item) = items.iter().find(|(id, _)| id.head() == first).unwrap();

    let mut b = graph();
    assert_eq!(
        b.receive(second_item.as_slice()).unwrap_err(),
        Error::MissingDependency(first)
    );
    let stored = b
        .storage()
        .get(&BlobHash::of_contents(second_item))
        .is_some();
    assert!(stored);
    b.receive(first_item.as_slice()).unwrap();
    let receipt = b.admit(&BlobHash::of_contents(second_item)).unwrap();
    assert_eq!(receipt.members.len(), 1);
    assert!(receipt.members[0].newly_delivered);
    // Receiving it again delivers nothing new and keeps the first payload ID.
    let again = b.admit(&BlobHash::of_contents(second_item)).unwrap();
    assert!(!again.members[0].newly_delivered);
    assert_eq!(again.members[0].payload, receipt.members[0].payload);
}

#[test]
fn materialization_sees_delivery_and_permission_changes() {
    let (alice, bob) = (key(1), key(2));
    let mut g = graph();
    let a1 = signed(&mut g, &alice, vec![], b"a1");
    let b1 = signed(&mut g, &bob, vec![a1], b"b1");
    let mut permissions = HashMap::new();
    permissions.insert(author(&alice), WritePermission::Unbounded);

    let mut cursor = MaterializationCursor::new();
    let target = g.history().view_point(&permissions);
    let changes = collect(g.history(), &mut cursor, &target);
    assert_eq!(
        changes,
        vec![(a1, true, Vis::After), (b1, true, Vis::Neither)]
    );

    permissions.insert(author(&bob), WritePermission::Unbounded);
    let target = g.history().view_point(&permissions);
    let changes = collect(g.history(), &mut cursor, &target);
    assert_eq!(changes, vec![(b1, false, Vis::After)]);

    permissions.insert(author(&bob), WritePermission::Denied);
    let target = g.history().view_point(&permissions);
    assert_eq!(
        collect(g.history(), &mut cursor, &target),
        vec![(b1, false, Vis::Before)]
    );
    assert_eq!(collect(g.history(), &mut cursor, &target), vec![]);
}

fn collect(
    history: &History,
    cursor: &mut MaterializationCursor,
    target: &ViewPoint,
) -> Vec<(CommitHash, bool, Vis)> {
    let mut changes = history.changes(cursor, target).unwrap();
    let mut out = vec![];
    loop {
        match history.next_changes(&mut changes, &mut out, 1).unwrap() {
            Batch::More => {}
            Batch::Complete(next) => {
                *cursor = next;
                return out
                    .into_iter()
                    .map(|c| (c.hash, c.newly_delivered, c.visibility))
                    .collect();
            }
        }
    }
}

#[test]
fn branching_from_inside_a_received_fragment() {
    // Found in review: Bob's fragment {m, g} carries only his signature on g, so a
    // commit on m whose canonical fragment is {m, h} has no evidence covering m.
    // It is certified as a standalone commit instead of staying a draft forever.
    let (alice, bob) = (key(1), key(2));
    let mut b = graph();
    let m = signed(&mut b, &bob, vec![], b"m");
    let g_payload = payload_at_depth(&bob, &[m], 1, "g");
    signed(&mut b, &bob, vec![m], &g_payload);
    let mut a = graph();
    sync(&mut b, &mut a, &ExportCursor::new());
    let h_payload = payload_at_depth(&alice, &[m], 1, "h");
    let h = signed(&mut a, &alice, vec![m], &h_payload);
    assert!(a.history().is_admitted(&h));
    let c = signed(&mut a, &alice, vec![h], b"c");
    assert!(a.history().is_admitted(&c));
    // Both sides converge.
    sync(&mut a, &mut b, &ExportCursor::new());
    assert!(b.history().is_admitted(&c));
}
