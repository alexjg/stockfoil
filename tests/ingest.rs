//! Contracts for stateless verification, atomic admission, and minimal-cover relay.
mod support;

use std::collections::BTreeSet;
use stockfoil::collections::HashMap;

use stockfoil::{
    CommitGraph, CommitHash, ExportError, FragmentRange, IngestError, IngestReport, ItemId,
    MaterializationCursor, UnverifiedItem, VerifiedItem, Vis, WritePermission,
};
use support::{
    at_depth, proof, seeded_author as author, signed_commit as item, signed_fragment as fragment,
};

fn assert_ingest_rejected_without_changes(
    graph: &mut CommitGraph<usize>,
    verified: VerifiedItem,
    expected: IngestError,
) {
    let members: Vec<_> = verified.commits().map(|meta| *meta.hash()).collect();
    let admitted: Vec<_> = members.iter().map(|hash| graph.has_commit(hash)).collect();
    let payloads: Vec<_> = members
        .iter()
        .map(|hash| graph.publishable_commit(hash).map(|(_, payload)| *payload))
        .collect();
    let cover = graph.minimal_cover();
    let view = graph.view_point();
    let signed = graph.signed_view_point();
    let heads = graph.heads().collect::<Vec<_>>();
    let pending = graph.pending_items();
    let missing = graph
        .missing_dependencies()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut callbacks = 0;
    assert_eq!(
        graph.ingest(verified, |_| {
            callbacks += 1;
            99
        }),
        Err(expected)
    );
    assert_eq!(callbacks, 0);
    assert_eq!(graph.pending_items(), pending);
    assert_eq!(
        graph
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        missing
    );
    assert_eq!(graph.minimal_cover(), cover);
    assert_eq!(graph.view_point(), view);
    assert_eq!(graph.signed_view_point(), signed);
    assert_eq!(graph.heads().collect::<Vec<_>>(), heads);
    for ((hash, admitted), payload) in members.iter().zip(admitted).zip(payloads) {
        assert_eq!(graph.has_commit(hash), admitted);
        assert_eq!(
            graph.publishable_commit(hash).map(|(_, payload)| *payload),
            payload
        );
    }
}

fn assert_report(mut report: IngestReport, mut admitted: Vec<ItemId>, pending: usize) {
    report.admitted.sort();
    admitted.sort();
    assert_eq!(report.admitted, admitted);
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    assert_eq!(report.pending, pending);
}

fn assert_missing<P: Clone>(graph: &CommitGraph<P>, missing: &[CommitHash]) {
    assert_eq!(
        graph
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        missing.iter().copied().collect(),
    );
}

fn sign<P: Clone>(graph: &mut CommitGraph<P>, seed: u8, hash: CommitHash) -> IngestReport {
    let evidence = proof(seed, hash);
    graph
        .ingest_signature(evidence.author, hash, evidence.signature)
        .unwrap()
}

fn assert_not_exportable<P: Clone>(graph: &CommitGraph<P>, id: ItemId) {
    assert_eq!(
        graph.export(&id).unwrap_err(),
        ExportError::NotInMinimalCover(id)
    );
}

#[test]
fn admit_verified_metadata_with_payload_storage_handles() {
    let meta = at_depth(1, vec![], 0);
    let incoming = item(&meta, 1);
    assert_eq!(incoming.dependencies().count(), 0);
    let verified = incoming.verify().unwrap();
    let mut graph = CommitGraph::new();
    graph.ingest(verified, |meta| *meta.payload_hash()).unwrap();
    assert!(graph.has_commit(meta.hash()));
    let (stored, handle) = graph.publishable_commit(meta.hash()).unwrap();
    assert_eq!(stored.hash(), meta.hash());
    assert_eq!(handle, meta.payload_hash());
    let cover = graph.minimal_cover();
    assert_eq!(cover.loose_commits.len(), 1);
    assert_eq!(cover.loose_commits[0].commit, *meta.hash());
    assert!(cover.fragments.is_empty());
    graph
        .export(&ItemId::Commit(*meta.hash()))
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn stateless_verification_precedes_dependency_admission() {
    let root = at_depth(1, vec![], 0);
    let child = at_depth(1, vec![*root.hash(), *root.hash()], 0);
    let incoming = item(&child, 1);
    assert_eq!(
        incoming.dependencies().collect::<Vec<_>>(),
        vec![*root.hash()]
    );
    let verified = incoming.verify().unwrap();
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        vec![*root.hash()]
    );
    let mut destination = CommitGraph::new();
    let before = destination.minimal_cover();
    let mut callbacks = 0;
    let view = destination.view_point();
    let signed = destination.signed_view_point();
    assert_report(
        destination
            .ingest(verified.clone(), |_| {
                callbacks += 1;
            })
            .unwrap(),
        vec![],
        1,
    );
    assert_eq!(callbacks, 1);
    assert_eq!(destination.pending_items(), 1);
    assert_missing(&destination, &[*root.hash()]);
    assert_eq!(destination.view_point(), view);
    assert_eq!(destination.signed_view_point(), signed);
    destination.traverse(|_, _| panic!("queued history must not be traversed"));
    destination.traverse_since(&[], |_, _| panic!("queued history must not be traversed"));
    destination
        .visit_changes(&MaterializationCursor::new(), &view, |_| {
            panic!("queued history must not be materialized")
        })
        .unwrap();
    assert_not_exportable(&destination, ItemId::Commit(*child.hash()));
    assert!(destination.publishable_commit(child.hash()).is_none());
    let mut relay = CommitGraph::new();
    assert_report(relay.merge(&destination), vec![], 0);
    assert!(!relay.has_commit(child.hash()));
    assert_eq!(destination.heads().count(), 0);
    assert_eq!(destination.minimal_cover(), before);
    assert!(!destination.has_commit(child.hash()));

    assert_report(
        destination
            .ingest(item(&root, 1).verify().unwrap(), |_| ())
            .unwrap(),
        vec![ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        0,
    );
    assert_missing(&destination, &[]);
    // Already processed certificates are still reported, without preparing payloads.
    assert_report(
        destination
            .ingest(verified, |_| panic!("duplicate payload"))
            .unwrap(),
        vec![ItemId::Commit(*child.hash())],
        0,
    );
    assert!(destination.has_commit(child.hash()));
    assert!(destination.publishable_commit(child.hash()).is_some());
}

#[test]
fn admission_refuses_unsigned_local_dependencies_even_in_the_working_view() {
    let root = at_depth(1, vec![], 0);
    let child = at_depth(1, vec![*root.hash()], 0);
    let verified = item(&child, 1).verify().unwrap();
    let mut destination = CommitGraph::new();
    destination.ingest_local_commit(root.clone(), 7);
    destination.set_write_permissions(&HashMap::from([(author(1), WritePermission::Unbounded)]));
    let working = destination.view_point();
    assert_eq!(working.heads(), &[*root.hash()]);
    assert_eq!(destination.heads().collect::<Vec<_>>(), vec![*root.hash()]);
    assert!(!destination.has_commit(root.hash()));
    let cover = destination.minimal_cover();
    assert_report(
        destination.ingest(verified.clone(), |_| 8).unwrap(),
        vec![],
        1,
    );
    assert_missing(&destination, &[*root.hash()]);
    assert_eq!(destination.view_point(), working);
    assert_eq!(destination.minimal_cover(), cover);
    assert!(!destination.has_commit(child.hash()));

    assert_report(
        sign(&mut destination, 1, *root.hash()),
        vec![ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        0,
    );
    assert!(destination.has_commit(root.hash()));
    assert!(destination.has_commit(child.hash()));
    destination
        .ingest(verified, |_| panic!("already woken"))
        .unwrap();
    assert_eq!(*destination.publishable_commit(root.hash()).unwrap().1, 7);
    assert_eq!(*destination.publishable_commit(child.hash()).unwrap().1, 8);
}

#[test]
fn fragment_boundary_is_a_real_admission_dependency_not_just_local_metadata() {
    let root = at_depth(2, vec![], 0);
    let boundary = at_depth(2, vec![*root.hash()], 1);
    let head = at_depth(1, vec![*boundary.hash()], 1);
    let base_range = FragmentRange {
        head: *boundary.hash(),
        boundary: BTreeSet::new(),
    };
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    let verified = fragment(&range, vec![head.clone()], 1).verify().unwrap();
    assert_eq!(verified.commits().count(), 1);
    assert_eq!(verified.signature_boundary().len(), 1);
    let mut receiver = CommitGraph::new();
    let mut callbacks = 0;
    assert_report(
        receiver
            .ingest(verified.clone(), |meta| {
                assert_eq!(meta.hash(), head.hash());
                callbacks += 1;
                3
            })
            .unwrap(),
        vec![],
        1,
    );
    assert_eq!(callbacks, 1);
    assert_missing(&receiver, &[*boundary.hash()]);
    receiver.ingest_local_commit(root.clone(), 1);
    receiver.ingest_local_commit(boundary.clone(), 2);
    assert!(!receiver.has_commit(boundary.hash()));
    assert_report(
        receiver
            .ingest(verified.clone(), |_| panic!("exact queued duplicate"))
            .unwrap(),
        vec![],
        1,
    );
    assert_missing(&receiver, &[*boundary.hash()]);
    let report = receiver
        .ingest(
            fragment(&base_range, vec![boundary.clone(), root.clone()], 2)
                .verify()
                .unwrap(),
            |_| panic!("existing local payloads must be preserved"),
        )
        .unwrap();
    assert_report(
        report,
        vec![ItemId::Fragment(base_range), ItemId::Fragment(range)],
        0,
    );
    assert!(receiver.has_commit(boundary.hash()));
    receiver
        .ingest(verified, |_| panic!("queued payload must be retained"))
        .unwrap();
    assert!(receiver.has_commit(head.hash()));
    assert_eq!(receiver.minimal_cover().fragments.len(), 2);
    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), receiver.minimal_cover());
    for (meta, payload) in [(&root, 1), (&boundary, 2), (&head, 3)] {
        assert!(relay.has_commit(meta.hash()));
        assert_eq!(*relay.publishable_commit(meta.hash()).unwrap().1, payload);
    }
}

#[test]
fn a_fragment_is_verified_as_one_item_before_any_commits_are_admitted() {
    let root = at_depth(1, vec![], 0);
    let head = at_depth(1, vec![*root.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let verified = fragment(&range, vec![head.clone(), root.clone()], 1)
        .verify()
        .unwrap();
    assert_eq!(verified.id(), &ItemId::Fragment(range.clone()));
    let mut graph = CommitGraph::new();
    let mut callbacks = Vec::new();
    graph
        .ingest(verified, |meta| {
            callbacks.push(*meta.hash());
            *meta.payload_hash()
        })
        .unwrap();
    assert_eq!(callbacks, vec![*root.hash(), *head.hash()]);
    assert!(graph.has_commit(root.hash()));
    assert!(graph.has_commit(head.hash()));
    let cover = graph.minimal_cover();
    assert!(cover.loose_commits.is_empty());
    assert_eq!(cover.fragments.len(), 1);
    assert_eq!(cover.fragments[0].range, range);
}

#[test]
fn minimal_fragment_relays_cross_author_evidence_but_never_arbitrary_subitems() {
    let a = at_depth(1, vec![], 0);
    let b = at_depth(2, vec![*a.hash()], 1);
    let c = at_depth(1, vec![*b.hash()], 2);
    let range = FragmentRange {
        head: *c.hash(),
        boundary: BTreeSet::new(),
    };
    let mut source = CommitGraph::new();
    source.ingest_local_commit(a.clone(), ());
    source.ingest_local_commit(b.clone(), ());
    sign(&mut source, 1, *b.hash());
    sign(&mut source, 2, *b.hash());
    source.set_write_permissions(&HashMap::from([
        (author(1), WritePermission::Unbounded),
        (author(2), WritePermission::Unbounded),
    ]));
    let captured = source.signed_view_point();
    assert_eq!(captured.heads(), &[*b.hash()]);
    assert!(source.has_commit(a.hash()));
    assert!(source.publishable_commit(a.hash()).is_some());
    assert_not_exportable(&source, ItemId::Commit(*a.hash()));
    assert!(source.minimal_cover().loose_commits.is_empty());
    let mut receiver = CommitGraph::new();
    receiver.merge(&source);
    assert_eq!(receiver.heads().collect::<Vec<_>>(), vec![*b.hash()]);

    source.ingest_local_commit(c.clone(), ());
    sign(&mut source, 1, *c.hash());
    let cover = source.minimal_cover();
    assert!(cover.loose_commits.is_empty());
    assert_eq!(cover.fragments.len(), 1);
    assert_eq!(cover.fragments[0].range, range);
    let incoming = source.export(&ItemId::Fragment(range.clone())).unwrap();
    assert_eq!(incoming.dependencies().count(), 0);
    receiver.ingest(incoming.verify().unwrap(), |_| ()).unwrap();
    for graph in [&source, &receiver] {
        for meta in [&a, &b, &c] {
            assert!(graph.has_commit(meta.hash()));
            assert!(graph.publishable_commit(meta.hash()).is_some());
            if meta.hash() == c.hash() {
                // A cover fragment's signed head can also be announced on its own.
                graph
                    .export(&ItemId::Commit(*meta.hash()))
                    .unwrap()
                    .verify()
                    .unwrap();
            } else {
                assert_not_exportable(graph, ItemId::Commit(*meta.hash()));
            }
        }
        assert_eq!(graph.minimal_cover(), cover);
    }
    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), cover);
    assert!(relay.has_commit(c.hash()));
    // Historical membership survives certificate replacement and relay even
    // though the captured heads are no longer independently exportable.
    for graph in [&source, &receiver, &relay] {
        let membership = graph.diff(&captured, &captured).unwrap();
        assert_eq!(membership.visibility(a.hash()), Ok(Vis::Both));
        assert_eq!(membership.visibility(b.hash()), Ok(Vis::Both));
        assert_eq!(membership.visibility(c.hash()), Ok(Vis::Neither));
    }
    assert_not_exportable(&relay, ItemId::Commit(*b.hash()));
    relay
        .export(&ItemId::Fragment(range))
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn owned_cover_snapshots_do_not_pin_old_commit_or_fragment_exports() {
    let root = at_depth(1, vec![], 0);
    let checkpoint = at_depth(1, vec![*root.hash()], 1);
    let head = at_depth(1, vec![*checkpoint.hash()], 2);
    let root_id = ItemId::Commit(*root.hash());
    let old_range = FragmentRange {
        head: *checkpoint.hash(),
        boundary: BTreeSet::new(),
    };
    let old_id = ItemId::Fragment(old_range.clone());
    let new_range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let mut graph = CommitGraph::new();
    graph
        .ingest(item(&root, 1).verify().unwrap(), |_| ())
        .unwrap();
    let loose_snapshot = graph.minimal_cover();
    graph.export(&root_id).unwrap().verify().unwrap();
    graph
        .ingest(
            fragment(&old_range, vec![checkpoint.clone(), root.clone()], 1)
                .verify()
                .unwrap(),
            |_| (),
        )
        .unwrap();
    let fragment_snapshot = graph.minimal_cover();
    graph.export(&old_id).unwrap().verify().unwrap();
    assert_not_exportable(&graph, root_id.clone());
    graph
        .ingest(
            fragment(
                &new_range,
                vec![head.clone(), checkpoint.clone(), root.clone()],
                1,
            )
            .verify()
            .unwrap(),
            |_| (),
        )
        .unwrap();
    assert_eq!(loose_snapshot.loose_commits[0].commit, *root.hash());
    assert_eq!(fragment_snapshot.fragments[0].range, old_range);
    assert_not_exportable(&graph, root_id.clone());
    assert_not_exportable(&graph, old_id.clone());
    // Historical targets are no longer accepted by separate signature ingestion.
    for meta in [&root, &checkpoint] {
        let evidence = proof(1, *meta.hash());
        assert_eq!(
            graph.ingest_signature(evidence.author, evidence.commit, evidence.signature),
            Err(stockfoil::SignatureError::NotPendingLocalCommit(
                *meta.hash()
            ))
        );
    }
    assert_not_exportable(&graph, root_id);
    assert_not_exportable(&graph, old_id);
    let cover = graph.minimal_cover();
    assert!(cover.loose_commits.is_empty());
    assert_eq!(cover.fragments.len(), 1);
    assert_eq!(cover.fragments[0].range, new_range);
    let exported = graph
        .export(&ItemId::Fragment(new_range))
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(exported.commits().count(), 3);
    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    assert_eq!(relay.minimal_cover(), cover);
    assert!(relay.has_commit(root.hash()));
    assert!(relay.has_commit(checkpoint.hash()));
    assert!(relay.has_commit(head.hash()));
}

#[test]
fn fragment_admission_preserves_pending_payloads_and_duplicate_receipts_are_noops() {
    let root = at_depth(1, vec![], 0);
    let middle = at_depth(1, vec![*root.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let incoming = fragment(&range, vec![head.clone(), middle.clone(), root.clone()], 1);
    let verified = incoming.verify().unwrap();
    let mut graph = CommitGraph::new();
    graph.ingest_local_commit(root.clone(), 7);
    graph.ingest_local_commit(middle.clone(), 8);
    let before = graph.view_point();
    let before_cover = graph.minimal_cover();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.ingest(verified.clone(), |_| panic!("payload preparation failed"))
        }))
        .is_err()
    );
    assert_eq!(graph.view_point(), before);
    assert_eq!(graph.minimal_cover(), before_cover);
    assert_eq!(graph.heads().collect::<Vec<_>>(), vec![*middle.hash()]);
    assert!(!graph.has_commit(middle.hash()));
    assert!(!graph.has_commit(head.hash()));

    let mut calls = 0;
    graph
        .ingest(verified.clone(), |meta| {
            assert_eq!(meta.hash(), head.hash());
            calls += 1;
            9
        })
        .unwrap();
    assert_eq!(calls, 1);
    for (meta, expected) in [(&root, 7), (&middle, 8), (&head, 9)] {
        assert_eq!(*graph.publishable_commit(meta.hash()).unwrap().1, expected);
        assert!(graph.has_commit(meta.hash()));
    }
    let cover = graph.minimal_cover();
    let view = graph.signed_view_point();
    graph
        .ingest(verified, |_| panic!("duplicate requested a payload"))
        .unwrap();
    assert_eq!(graph.minimal_cover(), cover);
    assert_eq!(graph.signed_view_point(), view);
    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    assert_eq!(relay.minimal_cover(), cover);
    for (meta, expected) in [(&root, 7), (&middle, 8), (&head, 9)] {
        assert_eq!(*relay.publishable_commit(meta.hash()).unwrap().1, expected);
    }
}

#[test]
fn payload_panic_after_partial_preparation_leaves_all_graph_state_unchanged() {
    let root = at_depth(1, vec![], 0);
    let middle = at_depth(1, vec![*root.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let verified = fragment(&range, vec![head.clone(), middle.clone(), root.clone()], 1)
        .verify()
        .unwrap();
    let mut graph = CommitGraph::new();
    graph
        .ingest(item(&root, 1).verify().unwrap(), |_| 7)
        .unwrap();
    let cover = graph.minimal_cover();
    let view = graph.signed_view_point();
    let mut calls = Vec::new();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.ingest(verified.clone(), |meta| {
                calls.push(*meta.hash());
                assert_ne!(meta.hash(), head.hash(), "second payload fails");
                8
            })
        }))
        .is_err()
    );
    assert_eq!(calls, vec![*middle.hash(), *head.hash()]);
    assert_eq!(graph.minimal_cover(), cover);
    assert_eq!(graph.signed_view_point(), view);
    assert_eq!(graph.heads().collect::<Vec<_>>(), vec![*root.hash()]);
    assert_eq!(*graph.publishable_commit(root.hash()).unwrap().1, 7);
    for meta in [&middle, &head] {
        assert!(!graph.has_commit(meta.hash()));
        assert!(graph.publishable_commit(meta.hash()).is_none());
    }
    graph
        .export(&ItemId::Commit(*root.hash()))
        .unwrap()
        .verify()
        .unwrap();
    graph.ingest(verified, |_| 8).unwrap();
    assert!(graph.has_commit(head.hash()));
}

#[test]
fn bypass_parent_below_boundary_is_an_external_parent_not_a_payload_or_proof_member() {
    // h -> m -> b -> x, with the bypass m -> x. The normalized h fragment
    // contains only m,h; b's authenticated history supplies x as well as b.
    let x = at_depth(2, vec![], 0);
    let b = at_depth(2, vec![*x.hash()], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    let incoming = fragment(&range, vec![head.clone(), middle.clone()], 1);
    let expected_dependencies: Vec<_> =
        BTreeSet::from([*b.hash(), *x.hash()]).into_iter().collect();
    assert_eq!(
        incoming.dependencies().collect::<Vec<_>>(),
        expected_dependencies
    );
    let UnverifiedItem::Fragment {
        commits,
        signatures,
        ..
    } = &incoming
    else {
        panic!("expected fragment");
    };
    assert_eq!(commits.len(), 2);
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].commit, *head.hash());

    // Neither decoder API receives x or b metadata, nor their author's proof.
    let streamed = UnverifiedItem::verify_fragment(
        range.clone(),
        signatures.clone(),
        BTreeSet::new(),
        commits.iter().cloned().map(Ok),
    )
    .unwrap();
    let verified = incoming.verify().unwrap();
    for verified in [&verified, &streamed] {
        assert_eq!(
            verified.external_parents().copied().collect::<Vec<_>>(),
            expected_dependencies
        );
        assert_eq!(
            verified.dependencies().copied().collect::<Vec<_>>(),
            expected_dependencies
        );
        assert_eq!(
            verified
                .commits()
                .map(|meta| *meta.hash())
                .collect::<Vec<_>>(),
            vec![*middle.hash(), *head.hash()]
        );
        assert_eq!(verified.signature_boundary().len(), 1);
        assert_eq!(
            verified.signature_boundary()[&author(1)],
            BTreeSet::from([*head.hash()])
        );
    }

    let mut receiver = CommitGraph::new();
    let before = receiver.view_point();
    let mut callbacks = Vec::new();
    assert_report(
        receiver
            .ingest(verified.clone(), |meta| {
                callbacks.push(*meta.hash());
                if meta.hash() == middle.hash() { 3 } else { 4 }
            })
            .unwrap(),
        vec![],
        1,
    );
    assert_eq!(callbacks, vec![*middle.hash(), *head.hash()]);
    assert_eq!(receiver.view_point(), before);
    assert_missing(&receiver, &expected_dependencies);
    assert!(!receiver.has_commit(middle.hash()));
    assert!(!receiver.has_commit(head.hash()));
    assert_report(
        receiver
            .ingest(item(&x, 2).verify().unwrap(), |_| 1)
            .unwrap(),
        vec![ItemId::Commit(*x.hash())],
        1,
    );
    receiver.ingest_local_commit(b.clone(), 2);
    assert_missing(&receiver, &[*b.hash()]);
    assert_report(
        receiver
            .ingest(verified.clone(), |_| panic!("queued duplicate"))
            .unwrap(),
        vec![],
        1,
    );
    let base_range = FragmentRange {
        head: *b.hash(),
        boundary: BTreeSet::new(),
    };
    receiver
        .ingest(
            fragment(&base_range, vec![b.clone(), x.clone()], 2)
                .verify()
                .unwrap(),
            |_| panic!("preserve existing dependency payloads"),
        )
        .unwrap();
    assert_eq!(receiver.pending_items(), 0);
    assert_missing(&receiver, &[]);
    assert!(receiver.has_commit(middle.hash()));
    assert!(receiver.has_commit(head.hash()));
    receiver
        .ingest(verified, |_| panic!("already admitted by wakeup"))
        .unwrap();

    let exported = receiver.export(&ItemId::Fragment(range.clone())).unwrap();
    let UnverifiedItem::Fragment {
        borrowed: _,
        range: exported_range,
        commits,
        signatures,
    } = &exported
    else {
        panic!("expected fragment");
    };
    assert_eq!(exported_range, &range);
    assert_eq!(
        commits.iter().map(|meta| *meta.hash()).collect::<Vec<_>>(),
        vec![*middle.hash(), *head.hash()]
    );
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].author, author(1));
    assert_eq!(signatures[0].commit, *head.hash());
    let exported = exported.verify().unwrap();
    assert_eq!(
        exported.external_parents().copied().collect::<Vec<_>>(),
        expected_dependencies
    );
    assert_eq!(
        exported.dependencies().copied().collect::<Vec<_>>(),
        expected_dependencies
    );

    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), receiver.minimal_cover());
    assert_eq!(relay.signed_view_point(), receiver.signed_view_point());
    for (meta, payload) in [(&x, 1), (&b, 2), (&middle, 3), (&head, 4)] {
        assert!(relay.has_commit(meta.hash()));
        assert_eq!(*relay.publishable_commit(meta.hash()).unwrap().1, payload);
    }
    relay
        .ingest(exported, |_| panic!("duplicate payload"))
        .unwrap();
    let relayed = relay
        .export(&ItemId::Fragment(range))
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(relayed.commits().count(), 2);
    assert_eq!(
        relayed.dependencies().copied().collect::<Vec<_>>(),
        expected_dependencies
    );
    let mut complete_relay = CommitGraph::new();
    complete_relay.merge(&relay);
    assert_eq!(complete_relay.minimal_cover(), receiver.minimal_cover());
    for meta in [&x, &b, &middle, &head] {
        assert!(complete_relay.has_commit(meta.hash()));
    }
}

#[test]
fn omitted_in_range_parent_passes_stateless_checks_but_not_admission_even_when_admitted() {
    // Unlike the valid bypass, x is NOT in b's history. A sender cannot omit
    // in-range x (and its author's signature), even if x is already admitted.
    let x = at_depth(2, vec![], 0);
    let b = at_depth(2, vec![], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    let incoming = UnverifiedItem::Fragment {
        borrowed: BTreeSet::new(),
        range: range.clone(),
        commits: vec![head.clone(), middle.clone()],
        signatures: vec![proof(1, *head.hash())],
    };
    let streamed = UnverifiedItem::verify_fragment(
        range.clone(),
        vec![proof(1, *head.hash())],
        BTreeSet::new(),
        [Ok(head.clone()), Ok(middle.clone())],
    )
    .unwrap();
    let verified = incoming
        .verify()
        .expect("auth and item-local structure are valid");
    assert_eq!(
        streamed.external_parents().collect::<Vec<_>>(),
        verified.external_parents().collect::<Vec<_>>()
    );
    assert_eq!(
        verified
            .external_parents()
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*b.hash(), *x.hash()])
    );
    assert_eq!(verified.commits().count(), 2);
    assert_eq!(verified.signature_boundary().len(), 1);
    let mut graph = CommitGraph::new();
    graph.ingest(item(&b, 2).verify().unwrap(), |_| 1).unwrap();
    let mut prepared = Vec::new();
    assert_report(
        graph
            .ingest(verified.clone(), |meta| {
                prepared.push(*meta.hash());
                99
            })
            .unwrap(),
        vec![],
        1,
    );
    assert_eq!(prepared, vec![*middle.hash(), *head.hash()]);
    assert_missing(&graph, &[*x.hash()]);
    assert_report(graph.ingest_local_commit(x.clone(), 2), vec![], 1);
    assert_report(
        graph
            .ingest(verified.clone(), |_| panic!("queued duplicate"))
            .unwrap(),
        vec![],
        1,
    );
    assert_missing(&graph, &[*x.hash()]);
    let report = sign(&mut graph, 2, *x.hash());
    assert_eq!(report.admitted, vec![ItemId::Commit(*x.hash())]);
    assert_eq!(report.pending, 0);
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].item, ItemId::Fragment(range.clone()));
    assert_eq!(report.rejected[0].error, IngestError::InvalidFragment);
    assert_eq!(graph.pending_items(), 0);
    assert_missing(&graph, &[]);
    assert!(graph.has_commit(x.hash()));
    graph.ingest_local_commit(middle.clone(), 3);
    assert!(!graph.has_commit(middle.hash()));
    assert_ingest_rejected_without_changes(
        &mut graph,
        verified.clone(),
        IngestError::InvalidFragment,
    );
    assert_ingest_rejected_without_changes(&mut graph, streamed, IngestError::InvalidFragment);
    assert!(!graph.has_commit(middle.hash()));
    assert!(!graph.has_commit(head.hash()));
    assert_not_exportable(&graph, ItemId::Fragment(range.clone()));

    // Supplying x and its proof instead is canonical; the earlier rejection
    // neither authenticated pending m nor changed its stored payload.
    let valid = UnverifiedItem::Fragment {
        borrowed: BTreeSet::new(),
        range,
        commits: vec![head.clone(), middle.clone(), x.clone()],
        signatures: vec![proof(1, *head.hash()), proof(2, *x.hash())],
    }
    .verify()
    .unwrap();
    graph
        .ingest(valid, |meta| {
            assert_eq!(meta.hash(), head.hash());
            4
        })
        .unwrap();
    assert_eq!(*graph.publishable_commit(middle.hash()).unwrap().1, 3);
    // Even an apparent duplicate must not bypass canonical validation.
    assert_ingest_rejected_without_changes(&mut graph, verified, IngestError::InvalidFragment);
}

#[test]
fn spurious_authenticated_boundary_is_rejected_before_payloads_or_state_changes() {
    let b = at_depth(1, vec![], 1);
    let unrelated = at_depth(2, vec![], 1);
    let head = at_depth(1, vec![*b.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash(), *unrelated.hash()]),
    };
    let verified = fragment(&range, vec![head.clone()], 1).verify().unwrap();
    assert_eq!(
        verified.external_parents().copied().collect::<Vec<_>>(),
        vec![*b.hash()]
    );
    assert_eq!(
        verified.dependencies().copied().collect::<BTreeSet<_>>(),
        range.boundary
    );
    let mut graph = CommitGraph::new();
    graph.ingest(item(&b, 1).verify().unwrap(), |_| 1).unwrap();
    graph
        .ingest(item(&unrelated, 2).verify().unwrap(), |_| 2)
        .unwrap();
    assert_ingest_rejected_without_changes(&mut graph, verified, IngestError::InvalidFragment);
    assert!(!graph.has_commit(head.hash()));
    assert_not_exportable(&graph, ItemId::Fragment(range));
}

#[test]
fn supplying_a_boundary_ancestor_as_a_member_is_not_normalized() {
    let x = at_depth(1, vec![], 0);
    let b = at_depth(1, vec![*x.hash()], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    // x is a reachable, low-depth member with valid signature coverage. Only
    // b's omitted metadata reveals that x belongs to the excluded history.
    let incoming = fragment(&range, vec![head.clone(), middle.clone(), x.clone()], 1);
    let verified = incoming.verify().unwrap();
    assert_eq!(verified.commits().count(), 3);
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        vec![*b.hash()]
    );
    let mut graph = CommitGraph::new();
    graph.ingest(item(&x, 1).verify().unwrap(), |_| 1).unwrap();
    graph.ingest(item(&b, 1).verify().unwrap(), |_| 2).unwrap();
    assert_ingest_rejected_without_changes(&mut graph, verified, IngestError::InvalidFragment);
    assert!(!graph.has_commit(middle.hash()));
    assert!(!graph.has_commit(head.hash()));
    graph
        .ingest(
            fragment(&range, vec![head.clone(), middle.clone()], 1)
                .verify()
                .unwrap(),
            |_| 3,
        )
        .unwrap();
    assert_eq!(*graph.publishable_commit(x.hash()).unwrap().1, 1);
}

#[test]
fn external_parent_walks_share_history_but_stop_at_the_first_checkpoint() {
    let older = at_depth(2, vec![], 1);
    let stop = at_depth(2, vec![*older.hash()], 1);
    let shared = at_depth(2, vec![*stop.hash()], 0);
    let x = at_depth(2, vec![*shared.hash()], 0);
    let y = at_depth(2, vec![*shared.hash(), *shared.hash()], 0);
    let b = at_depth(2, vec![*x.hash(), *y.hash()], 1);
    // Both low-depth external parents are referenced by an interior member. Their
    // walks overlap, and neither may continue through stop to older.
    let middle = at_depth(1, vec![*b.hash(), *x.hash(), *y.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash(), *stop.hash()]),
    };
    let members = vec![head.clone(), middle.clone()];
    let mut graph = CommitGraph::new();
    for meta in [&older, &stop, &shared, &x, &y, &b] {
        graph
            .ingest(item(meta, 2).verify().unwrap(), |_| 1)
            .unwrap();
    }

    // All claimed dependencies are admitted ancestors, but older is not a
    // depth-stopped boundary. Exact boundary validation must still reject it.
    let mut spurious = range.clone();
    spurious.boundary.insert(*older.hash());
    assert_ingest_rejected_without_changes(
        &mut graph,
        fragment(&spurious, members.clone(), 1).verify().unwrap(),
        IngestError::InvalidFragment,
    );
    let mut missing = range.clone();
    missing.boundary.remove(stop.hash());
    assert_ingest_rejected_without_changes(
        &mut graph,
        fragment(&missing, members.clone(), 1).verify().unwrap(),
        IngestError::InvalidFragment,
    );

    let incoming = fragment(&range, members, 1).verify().unwrap();
    assert_eq!(
        incoming.external_parents().copied().collect::<Vec<_>>(),
        BTreeSet::from([*b.hash(), *x.hash(), *y.hash()])
            .into_iter()
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        incoming.dependencies().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([*b.hash(), *stop.hash(), *x.hash(), *y.hash()]),
    );
    let mut applied = Vec::new();
    graph
        .ingest(incoming, |meta| {
            applied.push(*meta.hash());
            2
        })
        .unwrap();
    assert_eq!(applied, vec![*middle.hash(), *head.hash()]);
    let exported = graph
        .export(&ItemId::Fragment(range))
        .unwrap()
        .verify()
        .unwrap();
    graph
        .ingest(exported, |_| panic!("duplicate payload"))
        .unwrap();
    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    assert_eq!(relay.minimal_cover(), graph.minimal_cover());
}

#[test]
fn reverse_chain_and_fan_in_drain_to_quiescence() {
    let root = at_depth(1, vec![], 0);
    let left = at_depth(1, vec![*root.hash()], 0);
    let right = at_depth(2, vec![*root.hash()], 0);
    let join = at_depth(1, vec![*left.hash(), *right.hash(), *left.hash()], 0);
    let tip = at_depth(1, vec![*join.hash()], 0);
    let mut graph = CommitGraph::new();
    for (index, (meta, seed)) in [(&tip, 1), (&join, 1), (&right, 2), (&left, 1)]
        .into_iter()
        .enumerate()
    {
        assert_report(
            graph
                .ingest(item(meta, seed).verify().unwrap(), |_| index)
                .unwrap(),
            vec![],
            index + 1,
        );
        assert_eq!(graph.pending_items(), index + 1);
        assert!(!graph.has_commit(meta.hash()));
    }
    assert_missing(
        &graph,
        &[*root.hash(), *left.hash(), *right.hash(), *join.hash()],
    );
    assert_eq!(graph.heads().count(), 0);
    assert_report(
        graph
            .ingest(item(&root, 1).verify().unwrap(), |_| 4)
            .unwrap(),
        [&root, &left, &right, &join, &tip]
            .map(|meta| ItemId::Commit(*meta.hash()))
            .to_vec(),
        0,
    );
    assert_missing(&graph, &[]);
    assert_eq!(graph.pending_items(), 0);
    assert_eq!(graph.heads().collect::<Vec<_>>(), vec![*tip.hash()]);
    for (meta, payload) in [(&tip, 0), (&join, 1), (&right, 2), (&left, 3), (&root, 4)] {
        assert!(graph.has_commit(meta.hash()));
        assert_eq!(*graph.publishable_commit(meta.hash()).unwrap().1, payload);
    }
    let mut visited = Vec::new();
    graph.traverse(|meta, _| {
        for parent in meta.parents() {
            assert!(visited.contains(parent));
        }
        visited.push(*meta.hash());
        std::ops::ControlFlow::Continue(())
    });
    assert_eq!(visited.len(), 5);
}

#[test]
fn local_fragment_signing_wakes_waiters_on_promoted_interior_members() {
    let root = at_depth(1, vec![], 0);
    let middle = at_depth(1, vec![*root.hash()], 0);
    let checkpoint = at_depth(1, vec![*middle.hash()], 1);
    let child = at_depth(2, vec![*middle.hash()], 0);
    let tip = at_depth(2, vec![*child.hash()], 0);
    let range = FragmentRange {
        head: *checkpoint.hash(),
        boundary: BTreeSet::new(),
    };
    let mut graph = CommitGraph::new();
    assert_report(
        graph
            .ingest(item(&tip, 2).verify().unwrap(), |_| 5)
            .unwrap(),
        vec![],
        1,
    );
    assert_report(
        graph
            .ingest(item(&child, 2).verify().unwrap(), |_| 4)
            .unwrap(),
        vec![],
        2,
    );
    for (meta, payload) in [(&root, 1), (&middle, 2), (&checkpoint, 3)] {
        assert_report(graph.ingest_local_commit(meta.clone(), payload), vec![], 2);
        assert!(!graph.has_commit(meta.hash()));
    }
    assert_missing(&graph, &[*middle.hash(), *child.hash()]);
    assert_report(
        sign(&mut graph, 1, *checkpoint.hash()),
        vec![
            ItemId::Fragment(range),
            ItemId::Commit(*child.hash()),
            ItemId::Commit(*tip.hash()),
        ],
        0,
    );
    assert_missing(&graph, &[]);
    for (meta, payload) in [
        (&root, 1),
        (&middle, 2),
        (&checkpoint, 3),
        (&child, 4),
        (&tip, 5),
    ] {
        assert!(graph.has_commit(meta.hash()));
        assert_eq!(*graph.publishable_commit(meta.hash()).unwrap().1, payload);
    }
}

#[test]
fn queued_payloads_survive_cloning_but_later_local_payloads_take_precedence() {
    let root = at_depth(1, vec![], 0);
    let child = at_depth(1, vec![*root.hash()], 0);
    let verified = item(&child, 1).verify().unwrap();
    let mut original = CommitGraph::new();
    assert_report(
        original.ingest(verified.clone(), |_| 99).unwrap(),
        vec![],
        1,
    );
    assert_report(
        original
            .ingest(verified.clone(), |_| panic!("queued duplicate"))
            .unwrap(),
        vec![],
        1,
    );
    let mut cloned = original.clone();
    assert_eq!(cloned.pending_items(), 1);
    assert_missing(&cloned, &[*root.hash()]);
    assert_report(original.ingest_local_commit(root.clone(), 7), vec![], 1);
    assert_report(original.ingest_local_commit(child.clone(), 8), vec![], 1);
    assert_report(
        original
            .ingest(verified, |_| panic!("local payload already delivered"))
            .unwrap(),
        vec![],
        1,
    );
    assert_report(
        sign(&mut original, 1, *root.hash()),
        vec![ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        0,
    );
    assert_eq!(*original.publishable_commit(child.hash()).unwrap().1, 8);
    assert_eq!(cloned.pending_items(), 1);
    assert!(!cloned.has_commit(root.hash()));
    assert!(!cloned.has_commit(child.hash()));
    assert_eq!(cloned.heads().count(), 0);
    assert_report(
        cloned
            .ingest(item(&root, 1).verify().unwrap(), |_| 17)
            .unwrap(),
        vec![ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        0,
    );
    assert_eq!(*cloned.publishable_commit(child.hash()).unwrap().1, 99);
    assert_eq!(*cloned.publishable_commit(root.hash()).unwrap().1, 17);
    assert_eq!(*original.publishable_commit(root.hash()).unwrap().1, 7);
    assert_eq!(original.pending_items(), 0);
}

#[test]
fn queueing_a_certificate_for_delivered_local_members_does_not_prepare_payloads() {
    let root = at_depth(1, vec![], 0);
    let child = at_depth(1, vec![*root.hash()], 0);
    let mut graph = CommitGraph::new();
    assert_report(graph.ingest_local_commit(root.clone(), 7), vec![], 0);
    assert_report(graph.ingest_local_commit(child.clone(), 8), vec![], 0);
    let before = graph.view_point();
    assert_report(
        graph
            .ingest(item(&child, 1).verify().unwrap(), |_| {
                panic!("already delivered local payload")
            })
            .unwrap(),
        vec![],
        1,
    );
    assert_eq!(graph.view_point(), before);
    assert_missing(&graph, &[*root.hash()]);
    assert!(!graph.has_commit(child.hash()));
    assert_report(
        sign(&mut graph, 1, *root.hash()),
        vec![ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        0,
    );
    assert_eq!(*graph.publishable_commit(root.hash()).unwrap().1, 7);
    assert_eq!(*graph.publishable_commit(child.hash()).unwrap().1, 8);
}

#[test]
fn panic_during_queued_payload_preparation_preserves_queue_and_graph() {
    let boundary = at_depth(1, vec![], 1);
    let middle = at_depth(1, vec![*boundary.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let other = at_depth(2, vec![*boundary.hash()], 0);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    let verified = fragment(&range, vec![head.clone(), middle.clone()], 1)
        .verify()
        .unwrap();
    let mut graph = CommitGraph::new();
    graph
        .ingest(item(&other, 2).verify().unwrap(), |_| 10)
        .unwrap();
    let before = graph.view_point();
    let signed = graph.signed_view_point();
    let cover = graph.minimal_cover();
    let mut calls = Vec::new();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.ingest(verified.clone(), |meta| {
                calls.push(*meta.hash());
                assert_ne!(meta.hash(), head.hash(), "second queued payload fails");
                99
            })
        }))
        .is_err()
    );
    assert_eq!(calls, vec![*middle.hash(), *head.hash()]);
    assert_eq!(graph.pending_items(), 1);
    assert_missing(&graph, &[*boundary.hash()]);
    assert_eq!(graph.view_point(), before);
    assert_eq!(graph.signed_view_point(), signed);
    assert_eq!(graph.minimal_cover(), cover);
    assert_eq!(graph.heads().count(), 0);
    for meta in [&middle, &head, &other] {
        assert!(!graph.has_commit(meta.hash()));
        assert!(graph.publishable_commit(meta.hash()).is_none());
    }
    calls.clear();
    assert_report(
        graph
            .ingest(verified, |meta| {
                calls.push(*meta.hash());
                7
            })
            .unwrap(),
        vec![],
        2,
    );
    assert_eq!(calls, vec![*middle.hash(), *head.hash()]);
    assert_report(
        graph
            .ingest(item(&boundary, 1).verify().unwrap(), |_| 1)
            .unwrap(),
        vec![
            ItemId::Commit(*boundary.hash()),
            ItemId::Commit(*other.hash()),
            ItemId::Fragment(range),
        ],
        0,
    );
    assert_eq!(*graph.publishable_commit(middle.hash()).unwrap().1, 7);
    assert_eq!(*graph.publishable_commit(head.hash()).unwrap().1, 7);
    assert_eq!(*graph.publishable_commit(other.hash()).unwrap().1, 10);
}

#[test]
fn distinct_queued_certificates_with_the_same_id_are_both_processed() {
    let boundary = at_depth(1, vec![], 1);
    let middle = at_depth(1, vec![*boundary.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    let first = fragment(&range, vec![head.clone(), middle.clone()], 1)
        .verify()
        .unwrap();
    let second = UnverifiedItem::Fragment {
        borrowed: BTreeSet::new(),
        range: range.clone(),
        commits: vec![head.clone(), middle.clone()],
        signatures: vec![proof(1, *head.hash()), proof(1, *middle.hash())],
    }
    .verify()
    .unwrap();
    let mut graph = CommitGraph::new();
    for (index, verified) in [first, second].into_iter().enumerate() {
        let mut calls = 0;
        assert_report(
            graph
                .ingest(verified.clone(), |_| {
                    calls += 1;
                    7
                })
                .unwrap(),
            vec![],
            index + 1,
        );
        assert_eq!(calls, 2);
        assert_report(
            graph
                .ingest(verified, |_| panic!("exact queued duplicate"))
                .unwrap(),
            vec![],
            index + 1,
        );
    }
    assert_report(
        graph
            .ingest(item(&boundary, 1).verify().unwrap(), |_| 1)
            .unwrap(),
        vec![
            ItemId::Commit(*boundary.hash()),
            ItemId::Fragment(range.clone()),
            ItemId::Fragment(range),
        ],
        0,
    );
    assert!(graph.has_commit(head.hash()));
}

#[test]
fn deferred_rejection_does_not_suppress_same_id_valid_item_or_other_ready_items() {
    // The malformed certificate omits x, which is not in b's history. Both
    // certificates have the same ItemId, so queue identity cannot be just ItemId.
    let x = at_depth(2, vec![], 0);
    let b = at_depth(2, vec![], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let other = at_depth(2, vec![*b.hash()], 0);
    let absent = at_depth(3, vec![], 0);
    let waiting = at_depth(3, vec![*absent.hash()], 0);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    let malformed = fragment(&range, vec![head.clone(), middle.clone()], 1)
        .verify()
        .unwrap();
    let valid = UnverifiedItem::Fragment {
        borrowed: BTreeSet::new(),
        range: range.clone(),
        commits: vec![head.clone(), middle.clone(), x.clone()],
        signatures: vec![proof(1, *head.hash()), proof(2, *x.hash())],
    }
    .verify()
    .unwrap();
    // Exercise receipt, merge, and local signing as draining/reporting entrypoints,
    // and both insertion orders for colliding identities.
    for trigger in ["ingest", "merge", "sign"] {
        for valid_first in [false, true] {
            let mut graph = CommitGraph::new();
            graph.ingest(item(&x, 2).verify().unwrap(), |_| 2).unwrap();
            let items = if valid_first {
                [valid.clone(), malformed.clone()]
            } else {
                [malformed.clone(), valid.clone()]
            };
            for (index, verified) in items.into_iter().enumerate() {
                assert_report(graph.ingest(verified, |_| 3).unwrap(), vec![], index + 1);
            }
            assert_report(
                graph
                    .ingest(item(&other, 2).verify().unwrap(), |_| 4)
                    .unwrap(),
                vec![],
                3,
            );
            assert_report(
                graph
                    .ingest(item(&waiting, 3).verify().unwrap(), |_| 5)
                    .unwrap(),
                vec![],
                4,
            );
            assert_missing(&graph, &[*b.hash(), *absent.hash()]);
            let report = match trigger {
                "merge" => {
                    let mut source = CommitGraph::new();
                    source.ingest(item(&b, 2).verify().unwrap(), |_| 1).unwrap();
                    graph.merge(&source)
                }
                "sign" => {
                    assert_report(graph.ingest_local_commit(b.clone(), 1), vec![], 4);
                    sign(&mut graph, 2, *b.hash())
                }
                _ => graph.ingest(item(&b, 2).verify().unwrap(), |_| 1).unwrap(),
            };
            let mut admitted = report.admitted;
            admitted.sort();
            let base = if trigger == "sign" {
                ItemId::Fragment(FragmentRange {
                    head: *b.hash(),
                    boundary: BTreeSet::new(),
                })
            } else {
                ItemId::Commit(*b.hash())
            };
            let mut expected = vec![
                base,
                ItemId::Commit(*other.hash()),
                ItemId::Fragment(range.clone()),
            ];
            expected.sort();
            assert_eq!(admitted, expected);
            assert_eq!(report.rejected.len(), 1);
            assert_eq!(report.rejected[0].item, ItemId::Fragment(range.clone()));
            assert_eq!(report.rejected[0].error, IngestError::InvalidFragment);
            assert_eq!(report.pending, 1);
            assert_eq!(graph.pending_items(), 1);
            assert_missing(&graph, &[*absent.hash()]);
            for (meta, payload) in [(&b, 1), (&x, 2), (&middle, 3), (&head, 3), (&other, 4)] {
                assert!(graph.has_commit(meta.hash()));
                assert_eq!(*graph.publishable_commit(meta.hash()).unwrap().1, payload);
            }
            assert!(!graph.has_commit(waiting.hash()));
            assert_ingest_rejected_without_changes(
                &mut graph,
                malformed.clone(),
                IngestError::InvalidFragment,
            );
            assert_report(
                graph
                    .ingest(item(&absent, 3).verify().unwrap(), |_| 6)
                    .unwrap(),
                vec![
                    ItemId::Commit(*absent.hash()),
                    ItemId::Commit(*waiting.hash()),
                ],
                0,
            );
            assert_eq!(*graph.publishable_commit(waiting.hash()).unwrap().1, 5);
        }
    }
}

#[test]
fn raw_stopped_walk_keeps_a_boundary_reached_through_excluded_history() {
    // h -> m -> b -> x -> old, plus m -> x. Stopping at b alone is wrong:
    // the bypass reaches old, even though both x and old are b's ancestors.
    let old = at_depth(2, vec![], 1);
    let x = at_depth(2, vec![*old.hash()], 0);
    let b = at_depth(2, vec![*x.hash()], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash(), *old.hash()]),
    };
    let mut graph = CommitGraph::new();
    for (range, members) in [
        (
            FragmentRange {
                head: *old.hash(),
                boundary: BTreeSet::new(),
            },
            vec![old.clone()],
        ),
        (
            FragmentRange {
                head: *b.hash(),
                boundary: BTreeSet::from([*old.hash()]),
            },
            vec![b.clone(), x.clone()],
        ),
    ] {
        graph
            .ingest(fragment(&range, members, 2).verify().unwrap(), |_| 1)
            .unwrap();
    }
    let missing_boundary = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    let malformed = fragment(&missing_boundary, vec![head.clone(), middle.clone()], 1)
        .verify()
        .unwrap();
    assert_ingest_rejected_without_changes(&mut graph, malformed, IngestError::InvalidFragment);

    let verified = fragment(&range, vec![head.clone(), middle.clone()], 1)
        .verify()
        .unwrap();
    let dependencies: Vec<_> = BTreeSet::from([*b.hash(), *old.hash(), *x.hash()])
        .into_iter()
        .collect();
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        dependencies
    );
    graph.ingest(verified, |_| 2).unwrap();
    let exported = graph.export(&ItemId::Fragment(range.clone())).unwrap();
    let UnverifiedItem::Fragment { commits, .. } = &exported else {
        panic!("expected fragment");
    };
    assert_eq!(
        exported
            .clone()
            .verify()
            .unwrap()
            .external_parents()
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*b.hash(), *x.hash()])
    );
    assert_eq!(
        commits.iter().map(|meta| *meta.hash()).collect::<Vec<_>>(),
        vec![*middle.hash(), *head.hash()]
    );
    assert_eq!(exported.id(), ItemId::Fragment(range));
    exported.verify().unwrap();
    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    assert_eq!(relay.minimal_cover(), graph.minimal_cover());
    for meta in [&old, &x, &b, &middle, &head] {
        assert!(relay.has_commit(meta.hash()));
    }
}
