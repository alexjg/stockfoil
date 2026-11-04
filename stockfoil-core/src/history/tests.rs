use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::collections::HashSet;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn author(k: &SigningKey) -> Author {
    Author::try_from(k.verifying_key().as_bytes().as_slice()).unwrap()
}

/// Publish and sign a commit whose payload is found so its depth is exactly `level`.
fn commit(
    h: &mut History,
    k: &SigningKey,
    parents: Vec<CommitHash>,
    level: usize,
    tag: &str,
) -> CommitHash {
    for i in 0u64.. {
        let mut b = h.begin_commit(author(k), parents.clone()).unwrap();
        b.update(format!("{tag}/{i}").as_bytes());
        let p = b.finish();
        if depth(&p.hash) == level {
            let hash = p.hash;
            h.publish_commit(p, i).unwrap();
            h.sign(
                author(k),
                hash,
                Signature::from(k.sign(hash.as_bytes()).to_bytes()),
            )
            .unwrap();
            return hash;
        }
    }
    unreachable!()
}

fn naive_ancestry(h: &History, idx: NodeIdx) -> HashSet<NodeIdx> {
    let mut seen = HashSet::new();
    let mut stack = vec![idx];
    while let Some(i) = stack.pop() {
        if seen.insert(i) {
            stack.extend(h.parent_nodes(i));
        }
    }
    seen
}

#[test]
fn fragment_members_are_ancestry_differences() {
    let (a, b) = (key(1), key(2));
    let mut h = History::new();
    let mut tips = vec![commit(&mut h, &a, vec![], 0, "root")];
    for i in 0..60 {
        let k = if i % 3 == 0 { &b } else { &a };
        let parents = if i % 7 == 0 && tips.len() > 1 {
            vec![tips[tips.len() - 1], tips[tips.len() - 2]]
        } else {
            vec![tips[tips.len() - 1 - (i % 2).min(tips.len() - 1)]]
        };
        let level = (i % 11 == 5) as usize;
        tips.push(commit(&mut h, k, parents, level, &format!("c{i}")));
    }
    for idx in (0..h.len()).map(|i| NodeIdx::new(i as u32)) {
        let level = h.node(idx).depth as usize;
        if level == 0 {
            continue;
        }
        let boundary: Vec<_> = h.fragment_boundary(idx, level).into_iter().collect();
        let mut expected: HashSet<_> = naive_ancestry(&h, idx);
        for &b in &boundary {
            for x in naive_ancestry(&h, b) {
                expected.remove(&x);
            }
        }
        let got: HashSet<_> = h.fragment_members(idx, &boundary).into_iter().collect();
        assert_eq!(got, expected);
    }
}

#[test]
fn deeper_fragments_retire_the_fragments_they_cover() {
    let a = key(1);
    let mut h = History::new();
    let root = commit(&mut h, &a, vec![], 0, "root");
    let d1 = commit(&mut h, &a, vec![root], 1, "d1");
    let mid = commit(&mut h, &a, vec![d1], 0, "mid");
    let keys = |h: &History| h.inventory.keys().copied().collect::<Vec<_>>();
    assert_eq!(
        keys(&h),
        vec![export::Key::Commit(mid), export::Key::Fragment(d1)]
    );
    let d2 = commit(&mut h, &a, vec![mid], 2, "d2");
    assert_eq!(keys(&h), vec![export::Key::Fragment(d2)]);
    assert!(h.nodes.iter().all(|n| n.in_fragment && n.admitted));
}

#[test]
fn an_author_reuses_one_chain_across_interleaved_authors() {
    let (a, b) = (key(1), key(2));
    let mut h = History::new();
    let a1 = commit(&mut h, &a, vec![], 0, "a1");
    let b1 = commit(&mut h, &b, vec![a1], 0, "b1");
    let a2 = commit(&mut h, &a, vec![b1], 0, "a2");
    let chain = |h: &History, x: &CommitHash| h.node(h.by_hash[x]).chain;
    assert_eq!(chain(&h, &a1), chain(&h, &a2));
    assert_ne!(chain(&h, &a1), chain(&h, &b1));
    // A fork by the same author needs a second chain.
    let fork = commit(&mut h, &a, vec![a1], 0, "fork");
    assert_ne!(chain(&h, &fork), chain(&h, &a2));
    assert!(h.is_ancestor(&a1, &fork));
    assert!(!h.is_ancestor(&a2, &fork));
    assert!(!h.is_ancestor(&a1, &a1));
}

#[test]
fn drafts_admit_standalone_when_fragment_evidence_lies_elsewhere() {
    // Found by fuzzing: `a` is admitted only through Alice's signature on `h1`.
    // Carol's depth-1 draft on `a` has the canonical fragment {a, d}, which no
    // signature covers; it must still be admitted, as a standalone commit.
    let (alice, carol) = (key(1), key(3));
    let mut h = History::new();
    let mut builder = h.begin_commit(author(&alice), vec![]).unwrap();
    builder.update(b"a");
    let a = builder.finish();
    let a_hash = a.hash;
    h.publish_commit(a, 0).unwrap();
    let h1 = commit(&mut h, &alice, vec![a_hash], 1, "h1");
    assert!(h.is_admitted(&a_hash) && h.is_admitted(&h1));
    let d = commit(&mut h, &carol, vec![a_hash], 1, "d");
    assert!(h.is_admitted(&d));
    assert!(h.inventory.contains_key(&export::Key::Commit(d)));
}

#[test]
fn unsigned_members_block_fragment_assembly_until_their_author_signs() {
    let (a, b) = (key(1), key(2));
    let mut h = History::new();
    // Bob's draft sits inside Alice's depth-1 fragment.
    let mut builder = h.begin_commit(author(&b), vec![]).unwrap();
    builder.update(b"bob");
    let bob = builder.finish();
    let bob_hash = bob.hash;
    h.publish_commit(bob, 0).unwrap();
    let head = commit(&mut h, &a, vec![bob_hash], 1, "head");
    assert!(!h.is_admitted(&head));
    let admitted = h
        .sign(
            author(&b),
            bob_hash,
            Signature::from(b.sign(bob_hash.as_bytes()).to_bytes()),
        )
        .unwrap();
    // Bob's commit is admitted standalone, which completes Alice's fragment.
    assert_eq!(admitted.len(), 2);
    assert!(h.is_admitted(&head));
    assert_eq!(h.inventory.len(), 1);
}

#[test]
fn shared_types_cross_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<History>();
    assert_send_sync::<crate::Admission>();
    assert_send_sync::<Changes>();
    assert_send_sync::<ExportPlan>();
}

#[test]
fn merging_many_forks_shares_clock_structure() {
    // Found in review: each merge cached a full copy of every chain merged so far.
    let a = key(1);
    let mut h = History::new();
    let publish = |h: &mut History, parents: Vec<CommitHash>, tag: String| {
        let mut b = h.begin_commit(author(&a), parents).unwrap();
        b.update(tag.as_bytes());
        let p = b.finish();
        let hash = p.hash;
        h.publish_commit(p, 0).unwrap();
        hash
    };
    let root = publish(&mut h, vec![], "root".into());
    let n = 500;
    let forks: Vec<_> = (0..n)
        .map(|i| publish(&mut h, vec![root], format!("f{i}")))
        .collect();
    let mut tip = forks[0];
    for (i, f) in forks.iter().enumerate().skip(1) {
        tip = publish(&mut h, vec![tip, *f], format!("m{i}"));
    }
    let mut seen = HashSet::new();
    for node in &h.nodes {
        if let Some(clock) = node.clock.get() {
            clock.for_each_entry(&mut |entry| seen.insert(entry));
        }
    }
    // A full copy per merge would be about n²/2 = 125,000 entries.
    assert!(
        seen.len() < 30 * h.len(),
        "{} clock entries for {} commits",
        seen.len(),
        h.len()
    );
    assert!(h.is_ancestor(&forks[n - 1], &tip) && h.is_ancestor(&root, &tip));
}

fn sign_commit(k: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(k.sign(hash.as_bytes()).to_bytes())
}

fn calc_hash(parents: &[CommitHash], author: &Author, payload: &[u8]) -> CommitHash {
    crate::meta::commit_hash(parents.iter(), author, blake3::hash(payload).as_bytes())
}

fn at_level(k: &SigningKey, parents: &[CommitHash], level: usize, tag: &str) -> Vec<u8> {
    for i in 0u64.. {
        let payload = format!("{tag}-{i}").into_bytes();
        let hash = calc_hash(parents, &author(k), &payload);
        if depth(&hash) == level {
            return payload;
        }
    }
    unreachable!()
}

fn test_format() -> crate::FormatId {
    crate::FormatId::new("stockfoil-test/v1").unwrap()
}

fn encode_test_item(meta: &crate::BundleMeta, payloads: &[&[u8]]) -> Vec<u8> {
    let mut item = crate::wire::encode_header(meta, &test_format());
    for p in payloads {
        item.extend((p.len() as u32).to_le_bytes());
        item.extend_from_slice(p);
    }
    item
}

fn receive_test_item(h: &mut History, item: &[u8]) -> Result<Receipt, Error> {
    use crate::{DecodeStatus, Next, ReceiveOptions, Reply, Request};
    let mut op = h.receive(ReceiveOptions::new(test_format()));
    let mut pos = 0usize;
    let mut ended = 0usize;
    loop {
        match op.next() {
            Next::Continue => continue,
            Next::Done(r) => return Ok(r.clone()),
            Next::Failed(e) => return Err(e.clone()),
            Next::NeedHistory => match h.advance(&mut op) {
                Next::Done(r) => return Ok(r.clone()),
                Next::Failed(e) => return Err(e.clone()),
                Next::Request(t, Request::Bind { meta }) => {
                    let ids: Vec<_> = (0..meta.len() as u64).collect();
                    op.reply(t, Reply::Bound(&ids)).unwrap();
                }
                _ => {}
            },
            Next::Request(t, Request::Read { offset, max }) => {
                let start = (offset as usize).min(item.len());
                let end = (start + max).min(item.len());
                op.reply(
                    t,
                    Reply::Read {
                        bytes: &item[start..end],
                        eof: end == item.len(),
                    },
                )
                .unwrap();
            }
            Next::Request(t, Request::Decode { meta, input, .. }) => {
                let members = meta.len();
                if ended == members {
                    op.reply(
                        t,
                        Reply::Decoded {
                            consumed: 0,
                            output: &[],
                            status: DecodeStatus::Finished,
                        },
                    )
                    .unwrap();
                } else {
                    let len = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
                    let out = input[pos + 4..pos + 4 + len].to_vec();
                    pos += 4 + len;
                    ended += 1;
                    let consumed = if ended == members {
                        std::mem::take(&mut pos)
                    } else {
                        0
                    };
                    op.reply(
                        t,
                        Reply::Decoded {
                            consumed,
                            output: &out,
                            status: DecodeStatus::EndMember,
                        },
                    )
                    .unwrap();
                }
            }
            Next::Request(t, Request::Bind { meta }) => {
                let ids: Vec<_> = (0..meta.len() as u64).collect();
                op.reply(t, Reply::Bound(&ids)).unwrap();
            }
            other => panic!("{other:?}"),
        }
    }
}

struct TestMember {
    parents: Vec<crate::ParentRef>,
    payload: Vec<u8>,
}

fn build_fragment(
    k: &SigningKey,
    boundary: &[CommitHash],
    members: &[TestMember],
) -> (Vec<u8>, Vec<CommitHash>) {
    use crate::meta::Evidence;
    use crate::{BundleMeta, FragmentRange, MemberShape, ParentRef};
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
        hashes.push(calc_hash(&parents, &author(k), &m.payload));
    }
    let head = *hashes.last().unwrap();
    let meta = BundleMeta {
        id: ItemId::Fragment(FragmentRange {
            head,
            boundary: boundary.iter().copied().collect(),
        }),
        members: members
            .iter()
            .map(|m| MemberShape {
                author: author(k),
                parents: m.parents.clone(),
            })
            .collect(),
        evidence: vec![Evidence {
            author: author(k),
            member: members.len() - 1,
            signature: sign_commit(k, &head),
        }],
    };
    let payloads: Vec<&[u8]> = members.iter().map(|m| m.payload.as_slice()).collect();
    (encode_test_item(&meta, &payloads), hashes)
}

#[test]
fn fragments_must_sit_exactly_above_their_boundary() {
    use crate::ParentRef;
    let alice = key(1);
    let mut receiver = History::new();
    let mut b0 = receiver.begin_commit(author(&alice), vec![]).unwrap();
    b0.update(b"c0");
    let p0 = b0.finish();
    let c0 = p0.hash;
    receiver.publish_commit(p0, 0).unwrap();
    receiver
        .sign(author(&alice), c0, sign_commit(&alice, &c0))
        .unwrap();
    let b = commit(&mut receiver, &alice, vec![c0], 1, "b");

    let x1 = TestMember {
        parents: vec![ParentRef::External(c0)],
        payload: b"x1".to_vec(),
    };
    let x1_hash = calc_hash(&[c0], &author(&alice), b"x1");
    let x2_payload = b"x2".to_vec();
    let x2_hash = calc_hash(&[x1_hash, b], &author(&alice), &x2_payload);
    let h_payload = at_level(&alice, &[x2_hash], 1, "h");

    // A member below the boundary is rejected.
    let c0_again = TestMember {
        parents: vec![],
        payload: b"c0".to_vec(),
    };
    let x1_inner = TestMember {
        parents: vec![ParentRef::Member(0)],
        payload: b"x1".to_vec(),
    };
    let x2 = TestMember {
        parents: vec![ParentRef::Member(1), ParentRef::External(b)],
        payload: x2_payload.clone(),
    };
    let h = TestMember {
        parents: vec![ParentRef::Member(2)],
        payload: h_payload.clone(),
    };
    let (bad, _) = build_fragment(&alice, &[b], &[c0_again, x1_inner, x2, h]);
    assert_eq!(
        receive_test_item(&mut receiver, &bad).unwrap_err(),
        Error::InvalidPlacement
    );
    assert!(receiver.lookup(&x1_hash).is_none());

    // The canonical fragment is admitted.
    let x2 = TestMember {
        parents: vec![ParentRef::Member(0), ParentRef::External(b)],
        payload: x2_payload,
    };
    let h = TestMember {
        parents: vec![ParentRef::Member(1)],
        payload: h_payload,
    };
    let (good, hashes) = build_fragment(&alice, &[b], &[x1, x2, h]);
    let receipt = receive_test_item(&mut receiver, &good).unwrap();
    assert_eq!(receipt.members.len(), 3);
    assert!(hashes.iter().all(|h| receiver.is_admitted(h)));
}

#[test]
fn redundant_ancestor_boundaries_are_part_of_the_canonical_range() {
    use crate::ParentRef;
    let alice = key(1);
    let mut receiver = History::new();
    let b0 = commit(&mut receiver, &alice, vec![], 1, "b0");
    let e = commit(&mut receiver, &alice, vec![b0], 0, "e");
    let b1 = commit(&mut receiver, &alice, vec![e], 1, "b1");

    let x_payload = at_level(&alice, &[b1, e], 0, "x");
    let x_hash = calc_hash(&[b1, e], &author(&alice), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let members = || {
        [
            TestMember {
                parents: vec![ParentRef::External(b1), ParentRef::External(e)],
                payload: x_payload.clone(),
            },
            TestMember {
                parents: vec![ParentRef::Member(0)],
                payload: h_payload.clone(),
            },
        ]
    };
    let (partial, _) = build_fragment(&alice, &[b1], &members());
    assert_eq!(
        receive_test_item(&mut receiver, &partial).unwrap_err(),
        Error::InvalidPlacement
    );
    let (canonical, hashes) = build_fragment(&alice, &[b0, b1], &members());
    receive_test_item(&mut receiver, &canonical).unwrap();
    assert!(hashes.iter().all(|h| receiver.is_admitted(h)));
}

#[test]
fn deep_external_parents_must_be_declared_boundaries() {
    use crate::ParentRef;
    let alice = key(1);
    let mut receiver = History::new();
    let b = commit(&mut receiver, &alice, vec![], 1, "b");
    let x_payload = b"x".to_vec();
    let x_hash = calc_hash(&[b], &author(&alice), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let (undeclared, _) = build_fragment(
        &alice,
        &[],
        &[
            TestMember {
                parents: vec![ParentRef::External(b)],
                payload: x_payload,
            },
            TestMember {
                parents: vec![ParentRef::Member(0)],
                payload: h_payload,
            },
        ],
    );
    assert!(matches!(
        receive_test_item(&mut receiver, &undeclared).unwrap_err(),
        Error::InvalidItem(_)
    ));
}

#[test]
fn signatures_must_cover_every_author() {
    use crate::meta::Evidence;
    use crate::{BundleMeta, FragmentRange, MemberShape, ParentRef};
    let (alice, bob) = (key(1), key(2));
    let x_payload = b"bob's".to_vec();
    let x_hash = calc_hash(&[], &author(&bob), &x_payload);
    let h_payload = at_level(&alice, &[x_hash], 1, "h");
    let h_hash = calc_hash(&[x_hash], &author(&alice), &h_payload);
    let meta = |evidence: Vec<Evidence>| BundleMeta {
        id: ItemId::Fragment(FragmentRange {
            head: h_hash,
            boundary: BTreeSet::new(),
        }),
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
        evidence,
    };
    let alice_sig = Evidence {
        author: author(&alice),
        member: 1,
        signature: sign_commit(&alice, &h_hash),
    };
    let bob_sig = Evidence {
        author: author(&bob),
        member: 0,
        signature: sign_commit(&bob, &x_hash),
    };
    let mut h = History::new();
    let item = encode_test_item(&meta(vec![alice_sig]), &[&x_payload, &h_payload]);
    assert_eq!(
        receive_test_item(&mut h, &item).unwrap_err(),
        Error::MissingSignature {
            author: author(&bob),
            member: 0
        }
    );
    let item = encode_test_item(&meta(vec![alice_sig, bob_sig]), &[&x_payload, &h_payload]);
    receive_test_item(&mut h, &item).unwrap();
    assert!(h.is_admitted(&h_hash));
}

#[test]
fn signatures_that_cover_nothing_are_not_kept() {
    use crate::meta::Evidence;
    use crate::{BundleMeta, MemberShape};
    let (alice, bob, carol) = (key(1), key(2), key(3));
    let c_payload = b"c".to_vec();
    let c = calc_hash(&[], &author(&alice), &c_payload);
    let row = |k: &SigningKey| Evidence {
        author: author(k),
        member: 0,
        signature: sign_commit(k, &c),
    };
    let mut evidence = vec![row(&alice), row(&alice)];
    evidence.extend((10..15).map(|i| row(&key(i))));
    let meta = BundleMeta {
        id: ItemId::Commit(c),
        members: vec![MemberShape {
            author: author(&alice),
            parents: vec![],
        }],
        evidence,
    };
    let mut h = History::new();
    receive_test_item(&mut h, &encode_test_item(&meta, &[&c_payload])).unwrap();
    let signers = |h: &History| {
        let plan = h.export(&ExportCursor::new());
        assert_eq!(plan.len(), 1);
        let item = h.export_item(&plan, 0).unwrap();
        let mut signers: Vec<_> = item.meta.evidence.iter().map(|e| e.author).collect();
        signers.sort();
        signers
    };
    assert_eq!(signers(&h), vec![author(&alice)]);

    let mut builder = h.begin_commit(author(&bob), vec![c]).unwrap();
    builder.update(&at_level(&bob, &[c], 1, "h"));
    let p = builder.finish();
    let head = p.hash;
    h.publish_commit(p, 1).unwrap();
    h.sign(author(&carol), head, sign_commit(&carol, &head))
        .unwrap();
    h.sign(author(&bob), head, sign_commit(&bob, &head))
        .unwrap();
    let mut expected = vec![author(&alice), author(&bob)];
    expected.sort();
    assert_eq!(signers(&h), expected);
}
