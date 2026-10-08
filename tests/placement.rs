//! Canonical fragment placement against admitted history, with hand-built items.
mod support;
use ed25519_dalek::SigningKey;
use std::collections::BTreeSet;
use stockfoil::*;
use support::*;

/// A commit whose depth is exactly `level`.
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

struct Member {
    parents: Vec<ParentRef>,
    payload: Vec<u8>,
}

/// Hand-build a fragment item signed by `k` on its head.
fn fragment(
    k: &SigningKey,
    boundary: &[CommitHash],
    members: &[Member],
) -> (Vec<u8>, Vec<CommitHash>) {
    let mut hashes: Vec<CommitHash> = vec![];
    for m in members {
        let parents: Vec<_> = m
            .parents
            .iter()
            .map(|p| match p {
                ParentRef::Member(i) => hashes[*i],
                ParentRef::External(h) => *h,
            })
            .collect();
        hashes.push(commit_hash(&parents, &author(k), &m.payload));
    }
    let head = *hashes.last().unwrap();
    let meta = BundleMeta {
        id: ItemId::Fragment(FragmentRange {
            head,
            boundary: boundary.iter().copied().collect::<BTreeSet<_>>(),
        }),
        shape: FragmentShape {
            members: members
                .iter()
                .map(|m| MemberShape {
                    author: author(k),
                    parents: m.parents.clone(),
                })
                .collect(),
        },
        evidence: vec![Evidence {
            author: author(k),
            member: members.len() - 1,
            signature: sign(k, &head),
        }],
    };
    let payloads: Vec<&[u8]> = members.iter().map(|m| m.payload.as_slice()).collect();
    (encode_item(&meta, &payloads), hashes)
}

/// Commit and sign directly on a graph, then copy everything to `to`.
fn share(from: &mut TestGraph, to: &mut TestGraph) {
    sync(from, to, &ExportCursor::new());
}

#[test]
fn fragments_must_sit_exactly_above_their_boundary() {
    let alice = key(1);
    // c0 <- b (depth 1); c0 <- x1; x1, b <- x2; x2 <- h (depth 1).
    let mut a = graph();
    let c0 = signed(&mut a, &alice, vec![], b"c0");
    let b = signed(&mut a, &alice, vec![c0], &at_level(&alice, &[c0], 1, "b"));
    let mut receiver = graph();
    share(&mut a, &mut receiver);

    let x1 = Member {
        parents: vec![ParentRef::External(c0)],
        payload: b"x1".to_vec(),
    };
    let x1_hash = commit_hash(&[c0], &author(&alice), b"x1");
    let x2_payload = b"x2".to_vec();
    let x2_hash = commit_hash(&[x1_hash, b], &author(&alice), &x2_payload);
    let h_payload = at_level(&alice, &[x2_hash], 1, "h");

    // A member below the boundary is rejected.
    let c0_again = Member {
        parents: vec![],
        payload: b"c0".to_vec(),
    };
    let x1_inner = Member {
        parents: vec![ParentRef::Member(0)],
        payload: b"x1".to_vec(),
    };
    let x2 = Member {
        parents: vec![ParentRef::Member(1), ParentRef::External(b)],
        payload: x2_payload.clone(),
    };
    let h = Member {
        parents: vec![ParentRef::Member(2)],
        payload: h_payload.clone(),
    };
    let (bad, _) = fragment(&alice, &[b], &[c0_again, x1_inner, x2, h]);
    assert_eq!(
        receiver.receive(bad.as_slice()).unwrap_err(),
        Error::InvalidPlacement
    );
    assert!(!receiver.history().lookup(&x1_hash).is_some());

    // The canonical fragment is admitted.
    let x2 = Member {
        parents: vec![ParentRef::Member(0), ParentRef::External(b)],
        payload: x2_payload,
    };
    let h = Member {
        parents: vec![ParentRef::Member(1)],
        payload: h_payload,
    };
    let (good, hashes) = fragment(&alice, &[b], &[x1, x2, h]);
    let receipt = receiver.receive(good.as_slice()).unwrap();
    assert_eq!(receipt.members.len(), 3);
    assert!(hashes.iter().all(|h| receiver.history().is_admitted(h)));
}

#[test]
fn redundant_ancestor_boundaries_are_part_of_the_canonical_range() {
    let alice = key(1);
    // b0 (depth 1) <- e (depth 0) <- b1 (depth 1); x has parents [b1, e]; h (depth 1) on x.
    let mut a = graph();
    let b0 = signed(&mut a, &alice, vec![], &at_level(&alice, &[], 1, "b0"));
    let e = signed(&mut a, &alice, vec![b0], &at_level(&alice, &[b0], 0, "e"));
    let b1 = signed(&mut a, &alice, vec![e], &at_level(&alice, &[e], 1, "b1"));
    let mut receiver = graph();
    share(&mut a, &mut receiver);

    let x_payload = at_level(&alice, &[b1, e], 0, "x");
    let x_hash = commit_hash(&[b1, e], &author(&alice), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let members = || {
        [
            Member {
                parents: vec![ParentRef::External(b1), ParentRef::External(e)],
                payload: x_payload.clone(),
            },
            Member {
                parents: vec![ParentRef::Member(0)],
                payload: h_payload.clone(),
            },
        ]
    };
    // Walking down from `e` reaches `b0`, so `{b1}` alone is not canonical.
    let (partial, _) = fragment(&alice, &[b1], &members());
    assert_eq!(
        receiver.receive(partial.as_slice()).unwrap_err(),
        Error::InvalidPlacement
    );
    let (canonical, hashes) = fragment(&alice, &[b0, b1], &members());
    receiver.receive(canonical.as_slice()).unwrap();
    assert!(hashes.iter().all(|h| receiver.history().is_admitted(h)));

    // The sender assembles exactly this range for its own signed head.
    let head = signed(&mut a, &alice, vec![b1, e], &x_payload);
    assert_eq!(head, x_hash);
    let h = signed(&mut a, &alice, vec![x_hash], &h_payload);
    let plan = a.export(&ExportCursor::new());
    let ranges: Vec<_> = (0..plan.len())
        .filter_map(|i| match plan.id(i).unwrap() {
            ItemId::Fragment(r) if r.head == h => Some(r.boundary.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ranges, vec![BTreeSet::from([b0, b1])]);
}

#[test]
fn deep_external_parents_must_be_declared_boundaries() {
    let alice = key(1);
    let mut a = graph();
    let b = signed(&mut a, &alice, vec![], &at_level(&alice, &[], 1, "b"));
    let mut receiver = graph();
    share(&mut a, &mut receiver);
    let x_payload = b"x".to_vec();
    let x_hash = commit_hash(&[b], &author(&alice), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let (undeclared, _) = fragment(
        &alice,
        &[],
        &[
            Member {
                parents: vec![ParentRef::External(b)],
                payload: x_payload,
            },
            Member {
                parents: vec![ParentRef::Member(0)],
                payload: h_payload,
            },
        ],
    );
    assert!(matches!(
        receiver.receive(undeclared.as_slice()).unwrap_err(),
        Error::InvalidItem(_)
    ));
}

#[test]
fn signatures_must_cover_every_author() {
    let (alice, bob) = (key(1), key(2));
    // A two-author fragment signed only by Alice is incomplete.
    let x_payload = b"bob's".to_vec();
    let x_hash = commit_hash(&[], &author(&bob), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let h_hash = commit_hash(&[x_hash], &author(&alice), &h_payload);
    let meta = |evidence: Vec<Evidence>| BundleMeta {
        id: ItemId::Fragment(FragmentRange {
            head: h_hash,
            boundary: BTreeSet::new(),
        }),
        shape: FragmentShape {
            members: vec![
                MemberShape {
                    author: author(&bob),
                    parents: vec![],
                },
                MemberShape {
                    author: author(&alice),
                    parents: vec![ParentRef::Member(0)],
                },
            ],
        },
        evidence,
    };
    let alice_sig = Evidence {
        author: author(&alice),
        member: 1,
        signature: sign(&alice, &h_hash),
    };
    let bob_sig = Evidence {
        author: author(&bob),
        member: 0,
        signature: sign(&bob, &x_hash),
    };
    let mut g = graph();
    let item = encode_item(&meta(vec![alice_sig]), &[&x_payload, &h_payload]);
    assert_eq!(
        g.receive(item.as_slice()).unwrap_err(),
        Error::MissingSignature {
            author: author(&bob),
            member: 0
        }
    );
    let item = encode_item(&meta(vec![alice_sig, bob_sig]), &[&x_payload, &h_payload]);
    g.receive(item.as_slice()).unwrap();
    assert!(g.history().is_admitted(&h_hash));
    // Re-export keeps both authors' evidence.
    let mut other = graph();
    sync(&mut g, &mut other, &ExportCursor::new());
    assert!(other.history().is_admitted(&x_hash));
}

#[test]
fn received_fragments_retire_covered_standalone_items() {
    let alice = key(1);
    let mut a = graph();
    let c0 = signed(&mut a, &alice, vec![], b"c0");
    let c1 = signed(&mut a, &alice, vec![c0], b"c1");
    let mut b = graph();
    let cursor = sync(&mut a, &mut b, &ExportCursor::new());
    assert_eq!(cursor.len(), 2);
    // A fragment over both arrives later from elsewhere.
    let h = signed(&mut a, &alice, vec![c1], &at_level(&alice, &[c1], 1, "h"));
    let plan = a.export(&cursor);
    let retired: BTreeSet<_> = plan.retired().iter().cloned().collect();
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
