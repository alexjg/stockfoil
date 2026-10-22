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
    for idx in (0..h.len()).map(|i| NodeIdx(i as u32)) {
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
fn forged_prepared_commits_are_rejected() {
    let (alice, bob) = (key(1), key(2));
    let mut h = History::new();
    let mut builder = h.begin_commit(author(&alice), vec![]).unwrap();
    builder.update(b"x");
    let mut p = builder.finish();
    p.author = author(&bob);
    assert!(matches!(h.publish_commit(p, 0), Err(Error::InvalidItem(_))));
    assert!(h.is_empty());
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
    let forks: Vec<_> = (0..n).map(|i| publish(&mut h, vec![root], format!("f{i}"))).collect();
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
    assert!(seen.len() < 30 * h.len(), "{} clock entries for {} commits", seen.len(), h.len());
    assert!(h.is_ancestor(&forks[n - 1], &tip) && h.is_ancestor(&root, &tip));
}
