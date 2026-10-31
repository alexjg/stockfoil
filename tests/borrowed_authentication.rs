//! Fragments that borrow authentication for history they cannot cover.
//!
//! Signatures are discarded once a later signature covers them, so a branch
//! from inside an older fragment can produce a fragment whose shared history
//! nobody holds direct evidence for. Such a fragment declares that history as
//! borrowed and is admitted once another verified item authenticates it.

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::ControlFlow,
};

use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
use stockfoil::{
    CommitGraph, CommitHash, CommitMeta, FragmentRange, ItemId, UnverifiedItem, VerificationError,
};
use support::{at_depth, key, proof, seeded_author, sign, signed_commit};

fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes().iter().take_while(|b| **b == 0).count()
}

fn fragment(
    head: &CommitMeta,
    boundary: &[&CommitMeta],
    commits: &[&CommitMeta],
    signers: &[(u8, &CommitMeta)],
    borrowed: &[&CommitMeta],
) -> UnverifiedItem {
    UnverifiedItem::Fragment {
        range: FragmentRange {
            head: *head.hash(),
            boundary: boundary.iter().map(|m| *m.hash()).collect(),
        },
        commits: commits.iter().map(|m| (*m).clone()).collect(),
        signatures: signers
            .iter()
            .map(|(seed, meta)| proof(*seed, *meta.hash()))
            .collect(),
        borrowed: borrowed.iter().map(|m| *m.hash()).collect(),
    }
}

fn payload(meta: &CommitMeta) -> CommitHash {
    *meta.hash()
}

/// The regression which motivated borrowing. Alice branches from a commit
/// while it is a head, but signs only after Bob's later fragment has covered
/// (and so pruned) the branch point's own signature.
#[test]
fn signing_after_the_branch_point_is_compressed_does_not_strand_a_local_draft() {
    let bob = 2;
    let r = at_depth(bob, [], 0);
    let c = at_depth(bob, [*r.hash()], 0);
    let mut bobs = CommitGraph::new();
    for meta in [&r, &c] {
        bobs.ingest_local_commit(meta.clone(), payload(meta));
        sign(&mut bobs, &key(bob), *meta.hash());
    }
    let mut alices = CommitGraph::new();
    alices.merge(&bobs);
    let x = at_depth(1, [*c.hash()], 1);
    alices.ingest_local_commit(x.clone(), payload(&x));

    let s = at_depth(bob, [*c.hash()], 0);
    let h = at_depth(bob, [*s.hash()], 1);
    bobs.ingest_local_commit(s.clone(), payload(&s));
    bobs.ingest_local_commit(h.clone(), payload(&h));
    sign(&mut bobs, &key(bob), *h.hash());
    assert_eq!(bobs.minimal_cover().fragments.len(), 1);
    alices.merge(&bobs);

    sign(&mut alices, &key(1), *x.hash());
    assert!(
        alices.has_commit(x.hash()),
        "signed draft must be published"
    );
    let range = FragmentRange {
        head: *x.hash(),
        boundary: BTreeSet::new(),
    };
    let item = alices
        .export(&ItemId::Fragment(range))
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(item.borrowed(), &BTreeSet::from([*c.hash()]));
    assert_eq!(
        item.unauthenticated().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([*r.hash(), *c.hash()])
    );

    // The cover as a whole authenticates every member, in either order.
    for reverse in [false, true] {
        let mut items: Vec<_> = alices
            .export_changes(&Default::default())
            .0
            .added
            .into_iter()
            .map(|item| item.verify().unwrap())
            .collect();
        if reverse {
            items.reverse();
        }
        let mut fresh = CommitGraph::new();
        for item in items {
            assert!(fresh.ingest(item, payload).unwrap().rejected.is_empty());
        }
        assert_eq!(fresh.pending_items(), 0);
        for meta in [&r, &c, &s, &h, &x] {
            assert!(fresh.has_commit(meta.hash()));
        }
        assert_eq!(fresh.minimal_cover(), alices.minimal_cover());
    }
}

#[test]
fn borrowed_regions_are_declared_and_checked() {
    let p = at_depth(2, [], 0);
    let q = at_depth(3, [*p.hash()], 0);
    let head = at_depth(1, [*q.hash()], 1);
    let all = [&p, &q, &head];

    // Undeclared gaps still fail with the missing author and commit.
    assert_eq!(
        fragment(&head, &[], &all, &[(1, &head)], &[])
            .verify()
            .unwrap_err(),
        VerificationError::MissingSignature {
            author: seeded_author(3),
            commit: *q.hash()
        }
    );
    // A region covers its tips' ancestry, but no descendants.
    assert_eq!(
        fragment(&head, &[], &all, &[(1, &head)], &[&p])
            .verify()
            .unwrap_err(),
        VerificationError::MissingSignature {
            author: seeded_author(3),
            commit: *q.hash()
        }
    );
    let verified = fragment(&head, &[], &all, &[(1, &head)], &[&q])
        .verify()
        .unwrap();
    assert_eq!(
        verified.unauthenticated().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([*p.hash(), *q.hash()])
    );
    assert_eq!(
        verified.authenticated().copied().collect::<Vec<_>>(),
        [*head.hash()]
    );
    assert!(verified.dependencies().next().is_none());

    // Covered members inside the region are still authenticated by the item.
    let verified = fragment(&head, &[], &all, &[(1, &head), (2, &p)], &[&q])
        .verify()
        .unwrap();
    assert_eq!(
        verified.unauthenticated().copied().collect::<Vec<_>>(),
        [*q.hash()]
    );

    // A certificate must authenticate its head, and tips must be members.
    for borrowed in [&[&head][..], &[&at_depth(4, [], 0)][..]] {
        assert_eq!(
            fragment(&head, &[], &all, &[(1, &head)], borrowed)
                .verify()
                .unwrap_err(),
            VerificationError::InvalidItem
        );
    }
}

/// Two fragments which each authenticate history the other borrows. Neither
/// needs the other to be admitted first, so arrival order is irrelevant.
#[test]
fn mutually_borrowing_fragments_admit_in_either_order() {
    let p = at_depth(2, [], 0);
    let q = at_depth(3, [], 0);
    let carol_tip = at_depth(3, [*q.hash()], 0);
    let left = at_depth(1, [*p.hash(), *carol_tip.hash()], 1);
    let bob_tip = at_depth(2, [*p.hash()], 0);
    let right = at_depth(4, [*bob_tip.hash(), *q.hash()], 1);
    // Left authenticates Carol's Q but borrows Bob's P; right is the converse.
    let left_item = || {
        fragment(
            &left,
            &[],
            &[&p, &q, &carol_tip, &left],
            &[(1, &left), (3, &carol_tip)],
            &[&p],
        )
        .verify()
        .unwrap()
    };
    let right_item = || {
        fragment(
            &right,
            &[],
            &[&p, &q, &bob_tip, &right],
            &[(4, &right), (2, &bob_tip)],
            &[&q],
        )
        .verify()
        .unwrap()
    };
    for reverse in [false, true] {
        let (first, second) = if reverse {
            (right_item(), left_item())
        } else {
            (left_item(), right_item())
        };
        let mut graph = CommitGraph::new();
        let report = graph.ingest(first, payload).unwrap();
        assert!(report.admitted.is_empty());
        assert_eq!(report.pending, 1);
        assert_eq!(graph.missing_authentication().count(), 1);
        let report = graph.ingest(second, payload).unwrap();
        assert_eq!(report.admitted.len(), 2, "reverse={reverse}");
        assert_eq!(report.pending, 0);
        assert!(graph.missing_authentication().next().is_none());
        for meta in [&p, &q, &carol_tip, &left, &bob_tip, &right] {
            assert!(graph.has_commit(meta.hash()));
        }
        let mut relay = CommitGraph::new();
        relay.merge(&graph);
        assert_eq!(relay.pending_items(), 0);
        assert_eq!(relay.minimal_cover(), graph.minimal_cover());
    }
}

/// The borrower admits a commit which the lender needs as a boundary. Waiting
/// for the lender's admission (rather than its authentication) would deadlock.
#[test]
fn a_borrower_can_admit_the_lenders_boundary() {
    let q = at_depth(3, [], 0);
    let x = at_depth(2, [], 1);
    let carol_tip = at_depth(3, [*q.hash()], 0);
    let lender_head = at_depth(1, [*x.hash(), *carol_tip.hash()], 1);
    let borrower_head = at_depth(4, [*x.hash(), *q.hash()], 2);
    let lender = || {
        fragment(
            &lender_head,
            &[&x],
            &[&q, &carol_tip, &lender_head],
            &[(1, &lender_head), (3, &carol_tip)],
            &[],
        )
        .verify()
        .unwrap()
    };
    let borrower = || {
        fragment(
            &borrower_head,
            &[],
            &[&x, &q, &borrower_head],
            &[(4, &borrower_head), (2, &x)],
            &[&q],
        )
        .verify()
        .unwrap()
    };
    assert_eq!(
        lender().dependencies().copied().collect::<Vec<_>>(),
        [*x.hash()]
    );
    for reverse in [false, true] {
        let (first, second) = if reverse {
            (borrower(), lender())
        } else {
            (lender(), borrower())
        };
        let mut graph = CommitGraph::new();
        assert_eq!(graph.ingest(first, payload).unwrap().pending, 1);
        let report = graph.ingest(second, payload).unwrap();
        assert_eq!(report.admitted.len(), 2, "reverse={reverse}");
        assert_eq!(report.pending, 0);
        assert!(graph.has_commit(lender_head.hash()));
        assert!(graph.has_commit(borrower_head.hash()));
    }
}

/// A proof only counts once the item holding it can be admitted, alone or in a
/// batch. Otherwise a graph could advertise a borrower whose proof it cannot
/// relay, leaving peers which sync from it stuck.
#[test]
fn a_borrower_waits_until_its_proof_can_be_admitted() {
    let q = at_depth(3, [], 0);
    let missing = at_depth(5, [], 1);
    let carol_tip = at_depth(3, [*q.hash()], 0);
    let lender_head = at_depth(1, [*missing.hash(), *carol_tip.hash()], 1);
    let borrower_head = at_depth(4, [*q.hash()], 1);
    let lender = fragment(
        &lender_head,
        &[&missing],
        &[&q, &carol_tip, &lender_head],
        &[(1, &lender_head), (3, &carol_tip)],
        &[],
    )
    .verify()
    .unwrap();
    let borrower = fragment(
        &borrower_head,
        &[],
        &[&q, &borrower_head],
        &[(4, &borrower_head)],
        &[&q],
    )
    .verify()
    .unwrap();
    let mut graph = CommitGraph::new();
    graph.ingest(lender, payload).unwrap();
    let report = graph.ingest(borrower, payload).unwrap();
    assert!(report.admitted.is_empty());
    assert_eq!(report.pending, 2);
    assert!(!graph.has_commit(borrower_head.hash()));
    // The proof is held, so only the lender's dependency is missing.
    assert!(graph.missing_authentication().next().is_none());
    assert_eq!(
        graph.missing_dependencies().copied().collect::<Vec<_>>(),
        [*missing.hash()]
    );
    // Nothing unadmittable is advertised.
    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    assert_eq!(relay.pending_items(), 0);

    let report = graph
        .ingest(signed_commit(&missing, 5).verify().unwrap(), payload)
        .unwrap();
    assert_eq!(report.admitted.len(), 3);
    assert_eq!(report.pending, 0);
    relay.merge(&graph);
    assert_eq!(relay.pending_items(), 0);
    assert!(relay.has_commit(borrower_head.hash()));
}

/// A lender rejected for its placement proves nothing to the batch. Its
/// borrower stays queued until a valid proof arrives.
#[test]
fn a_rejected_lender_does_not_admit_its_borrower() {
    let q = at_depth(3, [], 0);
    let x = at_depth(2, [], 1);
    let stray = at_depth(6, [], 0);
    let carol_tip = at_depth(3, [*q.hash()], 0);
    let borrower_head = at_depth(4, [*x.hash(), *q.hash()], 2);
    // An extra external parent outside the declared boundary's history.
    let bad_head = at_depth(1, [*x.hash(), *carol_tip.hash(), *stray.hash()], 1);
    let good_head = at_depth(1, [*x.hash(), *carol_tip.hash()], 1);
    let lender = |head: &CommitMeta| {
        fragment(
            head,
            &[&x],
            &[&q, &carol_tip, head],
            &[(1, head), (3, &carol_tip)],
            &[],
        )
        .verify()
        .unwrap()
    };
    let borrower = fragment(
        &borrower_head,
        &[],
        &[&x, &q, &borrower_head],
        &[(4, &borrower_head), (2, &x)],
        &[&q],
    )
    .verify()
    .unwrap();
    let mut graph = CommitGraph::new();
    graph
        .ingest(signed_commit(&stray, 6).verify().unwrap(), payload)
        .unwrap();
    graph.ingest(borrower, payload).unwrap();
    let bad = lender(&bad_head);
    let bad_id = bad.id().clone();
    let report = graph.ingest(bad, payload).unwrap();
    assert!(report.admitted.is_empty());
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].item, bad_id);
    assert_eq!(
        report.rejected[0].error,
        stockfoil::IngestError::InvalidFragment
    );
    assert_eq!(report.pending, 1);
    assert!(!graph.has_commit(x.hash()));
    assert_eq!(
        graph.missing_authentication().copied().collect::<Vec<_>>(),
        [*q.hash()]
    );

    let report = graph.ingest(lender(&good_head), payload).unwrap();
    assert_eq!(report.admitted.len(), 2);
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 0);
    assert!(graph.has_commit(borrower_head.hash()));
    assert!(!graph.has_commit(bad_head.hash()));
}

// Replicas branching from arbitrary delivered commits, signing lazily and in
// any order, and merging at random. After every step each replica's cover must
// be verifiable and, taken as a whole, authenticate everything it advertises.
// Once all drafts are signed and replicas exchange covers, nothing may remain
// pending anywhere. Debug builds verify signatures slowly; for a longer run use
// `STOCKFOIL_BORROW_SEEDS=64 cargo test --release --test borrowed_authentication`.
#[test]
fn arbitrary_branch_points_keep_covers_self_authenticating() {
    const REPLICAS: usize = 3;
    const STEPS: usize = 80;
    let seeds: u64 = std::env::var("STOCKFOIL_BORROW_SEEDS")
        .ok()
        .and_then(|seeds| seeds.parse().ok())
        .unwrap_or(3);
    let mut borrowing = 0;
    for seed in 0..seeds {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut graphs: Vec<CommitGraph<CommitHash>> =
            (0..REPLICAS).map(|_| CommitGraph::new()).collect();
        let mut drafts: Vec<Vec<CommitHash>> = vec![Vec::new(); REPLICAS];
        for step in 0..STEPS {
            let replica = rng.random_range(0..REPLICAS);
            match rng.random_range(0..10) {
                0..=3 => {
                    let graph = &graphs[replica];
                    let delivered = delivered(graph);
                    // Favour branching from inside other authors' published
                    // history: commits which already have children, so are
                    // not heads, and whose own signatures may be long gone.
                    let author = seeded_author(replica as u8 + 1);
                    let interior: Vec<_> = delivered
                        .iter()
                        .copied()
                        .filter(|hash| {
                            graph.children(hash).next().is_some()
                                && graph
                                    .publishable_commit(hash)
                                    .is_some_and(|(meta, _)| meta.author() != &author)
                        })
                        .collect();
                    let mut parents = Vec::new();
                    if !delivered.is_empty() {
                        for _ in 0..rng.random_range(1..=2) {
                            let pool = if !interior.is_empty() && rng.random_bool(0.5) {
                                &interior
                            } else {
                                &delivered
                            };
                            let parent = pool[rng.random_range(0..pool.len())];
                            if !parents.contains(&parent) {
                                parents.push(parent);
                            }
                        }
                    }
                    let depth = match rng.random_range(0..40) {
                        0 => 2,
                        1..=14 => 1,
                        _ => 0,
                    };
                    let meta = at_depth(replica as u8 + 1, parents, depth);
                    graphs[replica].ingest_local_commit(meta.clone(), *meta.hash());
                    drafts[replica].push(*meta.hash());
                }
                4..=6 => {
                    if !drafts[replica].is_empty() {
                        let index = rng.random_range(0..drafts[replica].len());
                        let hash = drafts[replica].remove(index);
                        sign_draft(&mut graphs[replica], replica, hash);
                    }
                }
                _ => {
                    let other = rng.random_range(0..REPLICAS);
                    if other != replica {
                        let (target, source) = pair(&mut graphs, replica, other);
                        if rng.random_bool(0.5) {
                            let report = target.merge(source);
                            assert!(report.rejected.is_empty());
                        } else {
                            // An interrupted sync: some of the cover, in any
                            // order. Items left queued, including borrowers
                            // whose lenders are stuck, must not be advertised.
                            let mut items = source.export_changes(&Default::default()).0.added;
                            items.shuffle(&mut rng);
                            items.truncate(rng.random_range(0..=items.len()));
                            for item in items {
                                let item = item.verify().unwrap();
                                let report = target
                                    .ingest(item, |meta| {
                                        source.publishable_commit(meta.hash()).unwrap().1.clone()
                                    })
                                    .unwrap();
                                assert!(report.rejected.is_empty());
                            }
                        }
                    }
                }
            }
            // Only the acting replica changed.
            borrowing +=
                assert_self_authenticating(&graphs[replica], &format!("seed={seed} step={step}"));
        }
        for (replica, pending) in drafts.iter_mut().enumerate() {
            for hash in pending.drain(..) {
                sign_draft(&mut graphs[replica], replica, hash);
            }
        }
        for _ in 0..2 {
            for target in 0..REPLICAS {
                for source in 0..REPLICAS {
                    if source != target {
                        let (target, source) = pair(&mut graphs, target, source);
                        target.merge(source);
                    }
                }
            }
        }
        let expected = delivered(&graphs[0]);
        for (replica, graph) in graphs.iter().enumerate() {
            assert_eq!(graph.pending_items(), 0, "seed={seed}");
            assert_eq!(delivered(graph), expected, "seed={seed} replica={replica}");
            for hash in &expected {
                assert!(
                    graph.has_commit(hash),
                    "seed={seed} replica={replica}: stranded draft"
                );
            }
            assert_self_authenticating(graph, &format!("seed={seed} final"));
        }
    }
    eprintln!("borrowing cover items observed: {borrowing}");
    assert!(
        borrowing > 0,
        "the fixture must exercise borrowed certificates"
    );
}

fn pair<T>(items: &mut [T], target: usize, source: usize) -> (&mut T, &T) {
    assert_ne!(target, source);
    if target < source {
        let (left, right) = items.split_at_mut(source);
        (&mut left[target], &right[0])
    } else {
        let (left, right) = items.split_at_mut(target);
        (&mut right[0], &left[source])
    }
}

// A later signature may already have covered (and so admitted) the draft.
fn sign_draft(graph: &mut CommitGraph<CommitHash>, replica: usize, hash: CommitHash) {
    if !graph.has_commit(&hash) {
        sign(graph, &key(replica as u8 + 1), hash);
    }
}

fn delivered(graph: &CommitGraph<CommitHash>) -> Vec<CommitHash> {
    let mut hashes = Vec::new();
    graph.traverse(|meta, _| {
        hashes.push(*meta.hash());
        ControlFlow::Continue(())
    });
    hashes.sort();
    hashes
}

// Returns the number of borrowing items in the cover.
fn assert_self_authenticating(graph: &CommitGraph<CommitHash>, context: &str) -> usize {
    let cover = graph.minimal_cover();
    let mut items = BTreeMap::new();
    for id in cover
        .loose_commits
        .iter()
        .map(|c| ItemId::Commit(c.commit))
        .chain(
            cover
                .fragments
                .iter()
                .map(|f| ItemId::Fragment(f.range.clone())),
        )
    {
        let item =
            graph.export(&id).unwrap().verify().unwrap_or_else(|e| {
                panic!("{context}: cover item {id:?} fails verification: {e:?}")
            });
        if let ItemId::Fragment(range) = &id {
            assert!(depth(&range.head) > 0);
        }
        items.insert(id, item);
    }
    // Relay the cover from empty in reverse order, without a scheduler.
    let borrowing = items
        .values()
        .filter(|item| !item.borrowed().is_empty())
        .count();
    let mut fresh = CommitGraph::new();
    for item in items.into_values().rev() {
        let report = fresh.ingest(item, |meta| *meta.hash()).unwrap();
        assert!(report.rejected.is_empty(), "{context}");
    }
    assert_eq!(
        fresh.pending_items(),
        0,
        "{context}: cover leaves items queued"
    );
    let mut admitted = Vec::new();
    graph.traverse(|meta, _| {
        if graph.has_commit(meta.hash()) {
            admitted.push(*meta.hash());
        }
        ControlFlow::Continue(())
    });
    for hash in admitted {
        assert!(fresh.has_commit(&hash), "{context}: {hash:?} not relayed");
    }
    assert_eq!(fresh.minimal_cover(), cover, "{context}");
    borrowing
}
