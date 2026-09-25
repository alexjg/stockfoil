//! Contracts for stateless verification, atomic admission, and minimal-cover relay.
use std::collections::{BTreeSet, HashMap};

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{
    Author, BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, ExportError,
    FragmentRange, IngestError, ItemId, Signature, SignatureEvidence, UnverifiedItem, VerifiedItem,
    WritePermission,
};

fn author(seed: u8) -> Author {
    Author::try_from(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes()
            .as_slice(),
    )
    .unwrap()
}

fn at_depth(seed: u8, parents: Vec<CommitHash>, wanted: usize) -> CommitMeta {
    let author = author(seed);
    (0u64..10_000_000)
        .map(|nonce| {
            CommitMeta::new(CommitContents {
                parents: parents.clone(),
                author,
                payload: BlobHash::of_contents(&nonce.to_le_bytes()),
            })
        })
        .find(|meta| {
            meta.hash()
                .as_bytes()
                .iter()
                .take_while(|byte| **byte == 0)
                .count()
                == wanted
        })
        .expect("find a real hash at the requested depth")
}

fn proof(seed: u8, hash: CommitHash) -> SignatureEvidence {
    SignatureEvidence {
        author: author(seed),
        commit: hash,
        signature: Signature::from(
            SigningKey::from_bytes(&[seed; 32])
                .sign(hash.as_bytes())
                .to_bytes(),
        ),
    }
}

fn item(meta: &CommitMeta, seed: u8) -> UnverifiedItem {
    UnverifiedItem::Commit {
        meta: meta.clone(),
        signatures: vec![proof(seed, *meta.hash())],
    }
}

fn fragment(range: &FragmentRange, commits: Vec<CommitMeta>, seed: u8) -> UnverifiedItem {
    // Fill transport prerequisites, but never normalize/filter the supplied
    // members: deliberately malformed fragments must remain malformed.
    let members: BTreeSet<_> = commits.iter().map(|meta| *meta.hash()).collect();
    let additional_dependencies = commits
        .iter()
        .flat_map(|meta| meta.parents().iter().copied())
        .filter(|hash| !members.contains(hash) && !range.boundary.contains(hash))
        .collect();
    UnverifiedItem::Fragment {
        range: range.clone(),
        additional_dependencies,
        commits,
        signatures: vec![proof(seed, range.head)],
    }
}

fn assert_ingest_rejected_without_changes(
    graph: &mut CommitGraph<usize>,
    verified: VerifiedItem,
    expected: IngestError,
) {
    let members: Vec<_> = verified.commits().map(|meta| *meta.hash()).collect();
    let admitted: Vec<_> = members
        .iter()
        .map(|hash| graph.has_admitted_commit(hash))
        .collect();
    let payloads: Vec<_> = members
        .iter()
        .map(|hash| graph.publishable_commit(hash).map(|(_, payload)| *payload))
        .collect();
    let cover = graph.minimal_cover();
    let view = graph.view_point();
    let signed = graph.signed_view_point();
    let heads = graph.delivered_heads().collect::<Vec<_>>();
    let mut callbacks = 0;
    assert_eq!(
        graph.ingest(verified, |_| {
            callbacks += 1;
            99
        }),
        Err(expected)
    );
    assert_eq!(callbacks, 0);
    assert_eq!(graph.minimal_cover(), cover);
    assert_eq!(graph.view_point(), view);
    assert_eq!(graph.signed_view_point(), signed);
    assert_eq!(graph.delivered_heads().collect::<Vec<_>>(), heads);
    for ((hash, admitted), payload) in members.iter().zip(admitted).zip(payloads) {
        assert_eq!(graph.has_admitted_commit(hash), admitted);
        assert_eq!(
            graph.publishable_commit(hash).map(|(_, payload)| *payload),
            payload
        );
    }
}

fn sign<P: Clone>(graph: &mut CommitGraph<P>, seed: u8, hash: CommitHash) {
    let evidence = proof(seed, hash);
    graph
        .ingest_signature(evidence.author, hash, evidence.signature)
        .unwrap();
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
    assert!(graph.has_admitted_commit(meta.hash()));
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
    assert_eq!(
        destination.ingest(verified.clone(), |_| {
            callbacks += 1;
        }),
        Err(IngestError::MissingDependency(*root.hash())),
    );
    assert_eq!(callbacks, 0);
    assert_eq!(destination.delivered_heads().count(), 0);
    assert_eq!(destination.minimal_cover(), before);
    assert!(!destination.has_admitted_commit(child.hash()));

    destination
        .ingest(item(&root, 1).verify().unwrap(), |_| ())
        .unwrap();
    // The same already-verified certificate is now admissible; no reverification.
    destination.ingest(verified, |_| ()).unwrap();
    assert!(destination.has_admitted_commit(child.hash()));
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
    assert_eq!(working.delivered_heads(), &[*root.hash()]);
    assert!(destination.resolve(&working).is_err());
    assert_eq!(
        destination.delivered_heads().collect::<Vec<_>>(),
        vec![*root.hash()]
    );
    assert!(!destination.has_admitted_commit(root.hash()));
    let cover = destination.minimal_cover();
    assert_eq!(
        destination.ingest(verified.clone(), |_| panic!(
            "dependency check must precede payloads"
        )),
        Err(IngestError::MissingDependency(*root.hash()))
    );
    assert_eq!(destination.view_point(), working);
    assert_eq!(destination.minimal_cover(), cover);
    assert!(!destination.has_admitted_commit(child.hash()));

    sign(&mut destination, 1, *root.hash());
    assert!(destination.has_admitted_commit(root.hash()));
    destination.ingest(verified, |_| 8).unwrap();
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
    assert_eq!(
        receiver.ingest(verified.clone(), |_| panic!("missing boundary")),
        Err(IngestError::MissingDependency(*boundary.hash()))
    );
    receiver.ingest_local_commit(root.clone(), 1);
    receiver.ingest_local_commit(boundary.clone(), 2);
    assert!(!receiver.has_admitted_commit(boundary.hash()));
    assert_eq!(
        receiver.ingest(verified.clone(), |_| panic!("unsigned boundary")),
        Err(IngestError::MissingDependency(*boundary.hash()))
    );
    receiver
        .ingest(
            fragment(&base_range, vec![boundary.clone(), root.clone()], 2)
                .verify()
                .unwrap(),
            |_| panic!("existing local payloads must be preserved"),
        )
        .unwrap();
    assert!(receiver.has_admitted_commit(boundary.hash()));
    receiver
        .ingest(verified, |meta| {
            assert_eq!(meta.hash(), head.hash());
            3
        })
        .unwrap();
    assert!(receiver.has_admitted_commit(head.hash()));
    assert_eq!(receiver.minimal_cover().fragments.len(), 2);
    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), receiver.minimal_cover());
    for (meta, payload) in [(&root, 1), (&boundary, 2), (&head, 3)] {
        assert!(relay.has_admitted_commit(meta.hash()));
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
    assert!(graph.has_admitted_commit(root.hash()));
    assert!(graph.has_admitted_commit(head.hash()));
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
    assert_eq!(captured.delivered_heads(), &[*b.hash()]);
    assert!(
        source
            .resolve(&captured)
            .unwrap()
            .visible(a.hash())
            .unwrap()
    );
    assert!(source.has_admitted_commit(a.hash()));
    assert!(source.publishable_commit(a.hash()).is_some());
    assert_not_exportable(&source, ItemId::Commit(*a.hash()));
    assert!(source.minimal_cover().loose_commits.is_empty());
    let mut receiver = CommitGraph::new();
    receiver.merge(&source);
    assert_eq!(
        receiver.delivered_heads().collect::<Vec<_>>(),
        vec![*b.hash()]
    );

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
            assert!(graph.has_admitted_commit(meta.hash()));
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
        let resolved = graph.resolve(&captured).unwrap();
        assert!(resolved.visible(a.hash()).unwrap());
        assert!(resolved.visible(b.hash()).unwrap());
        assert!(!resolved.visible(c.hash()).unwrap());
        assert_eq!(graph.minimal_cover(), cover);
    }
    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), cover);
    assert!(relay.has_admitted_commit(c.hash()));
    assert!(relay.resolve(&captured).unwrap().visible(a.hash()).unwrap());
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
    assert!(relay.has_admitted_commit(root.hash()));
    assert!(relay.has_admitted_commit(checkpoint.hash()));
    assert!(relay.has_admitted_commit(head.hash()));
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
    assert_eq!(
        graph.delivered_heads().collect::<Vec<_>>(),
        vec![*middle.hash()]
    );
    assert!(!graph.has_admitted_commit(middle.hash()));
    assert!(!graph.has_admitted_commit(head.hash()));

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
        assert!(graph.has_admitted_commit(meta.hash()));
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
    assert_eq!(
        graph.delivered_heads().collect::<Vec<_>>(),
        vec![*root.hash()]
    );
    assert_eq!(*graph.publishable_commit(root.hash()).unwrap().1, 7);
    for meta in [&middle, &head] {
        assert!(!graph.has_admitted_commit(meta.hash()));
        assert!(graph.publishable_commit(meta.hash()).is_none());
    }
    graph
        .export(&ItemId::Commit(*root.hash()))
        .unwrap()
        .verify()
        .unwrap();
    graph.ingest(verified, |_| 8).unwrap();
    assert!(graph.has_admitted_commit(head.hash()));
}

#[test]
fn bypass_parent_below_boundary_is_an_explicit_dependency_not_a_payload_or_proof_member() {
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
        additional_dependencies,
        commits,
        signatures,
        ..
    } = &incoming
    else {
        panic!("expected fragment");
    };
    assert_eq!(additional_dependencies, &BTreeSet::from([*x.hash()]));
    assert_eq!(commits.len(), 2);
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].commit, *head.hash());

    // Neither decoder API receives x or b metadata, nor their author's proof.
    let streamed = UnverifiedItem::verify_fragment(
        range.clone(),
        additional_dependencies.clone(),
        signatures.clone(),
        commits.iter().cloned().map(Ok),
    )
    .unwrap();
    let verified = incoming.verify().unwrap();
    for verified in [&verified, &streamed] {
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
    assert_ingest_rejected_without_changes(
        &mut receiver,
        verified.clone(),
        IngestError::MissingDependency(expected_dependencies[0]),
    );
    receiver
        .ingest(item(&x, 2).verify().unwrap(), |_| 1)
        .unwrap();
    receiver.ingest_local_commit(b.clone(), 2);
    assert_ingest_rejected_without_changes(
        &mut receiver,
        verified.clone(),
        IngestError::MissingDependency(*b.hash()),
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
    let mut callbacks = Vec::new();
    receiver
        .ingest(verified, |meta| {
            callbacks.push(*meta.hash());
            if meta.hash() == middle.hash() { 3 } else { 4 }
        })
        .unwrap();
    assert_eq!(callbacks, vec![*middle.hash(), *head.hash()]);

    let exported = receiver.export(&ItemId::Fragment(range.clone())).unwrap();
    let UnverifiedItem::Fragment {
        range: exported_range,
        additional_dependencies,
        commits,
        signatures,
    } = &exported
    else {
        panic!("expected fragment");
    };
    assert_eq!(exported_range, &range);
    assert_eq!(additional_dependencies, &BTreeSet::from([*x.hash()]));
    assert_eq!(
        commits.iter().map(|meta| *meta.hash()).collect::<Vec<_>>(),
        vec![*middle.hash(), *head.hash()]
    );
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].author, author(1));
    assert_eq!(signatures[0].commit, *head.hash());
    let exported = exported.verify().unwrap();
    assert_eq!(
        exported.dependencies().copied().collect::<Vec<_>>(),
        expected_dependencies
    );

    let mut relay = CommitGraph::new();
    relay.merge(&receiver);
    assert_eq!(relay.minimal_cover(), receiver.minimal_cover());
    assert_eq!(relay.signed_view_point(), receiver.signed_view_point());
    for (meta, payload) in [(&x, 1), (&b, 2), (&middle, 3), (&head, 4)] {
        assert!(relay.has_admitted_commit(meta.hash()));
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
        assert!(complete_relay.has_admitted_commit(meta.hash()));
    }
}

#[test]
fn declaring_an_in_range_parent_as_extra_passes_stateless_checks_but_not_admission() {
    // Unlike the valid bypass, x is NOT in b's history. A sender cannot omit
    // in-range x (and its author's signature) merely by declaring it an extra.
    let x = at_depth(2, vec![], 0);
    let b = at_depth(2, vec![], 1);
    let middle = at_depth(1, vec![*b.hash(), *x.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*b.hash()]),
    };
    let incoming = UnverifiedItem::Fragment {
        range: range.clone(),
        additional_dependencies: BTreeSet::from([*x.hash()]),
        commits: vec![head.clone(), middle.clone()],
        signatures: vec![proof(1, *head.hash())],
    };
    let verified = incoming
        .verify()
        .expect("auth and declared closure are valid");
    assert_eq!(verified.commits().count(), 2);
    assert_eq!(verified.signature_boundary().len(), 1);
    let mut graph = CommitGraph::new();
    graph.ingest(item(&b, 2).verify().unwrap(), |_| 1).unwrap();
    assert_ingest_rejected_without_changes(
        &mut graph,
        verified.clone(),
        IngestError::MissingDependency(*x.hash()),
    );
    graph.ingest_local_commit(x.clone(), 2);
    assert_ingest_rejected_without_changes(
        &mut graph,
        verified.clone(),
        IngestError::MissingDependency(*x.hash()),
    );
    sign(&mut graph, 2, *x.hash());
    graph.ingest_local_commit(middle.clone(), 3);
    assert!(!graph.has_admitted_commit(middle.hash()));
    assert_ingest_rejected_without_changes(
        &mut graph,
        verified.clone(),
        IngestError::InvalidFragment,
    );
    assert!(!graph.has_admitted_commit(middle.hash()));
    assert!(!graph.has_admitted_commit(head.hash()));
    assert_not_exportable(&graph, ItemId::Fragment(range.clone()));

    // Supplying x and its proof instead is canonical; the earlier rejection
    // neither authenticated pending m nor changed its stored payload.
    let valid = UnverifiedItem::Fragment {
        range,
        additional_dependencies: BTreeSet::new(),
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
    let mut graph = CommitGraph::new();
    graph.ingest(item(&b, 1).verify().unwrap(), |_| 1).unwrap();
    graph
        .ingest(item(&unrelated, 2).verify().unwrap(), |_| 2)
        .unwrap();
    assert_ingest_rejected_without_changes(&mut graph, verified, IngestError::InvalidFragment);
    assert!(!graph.has_admitted_commit(head.hash()));
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
    assert!(!graph.has_admitted_commit(middle.hash()));
    assert!(!graph.has_admitted_commit(head.hash()));
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
    let UnverifiedItem::Fragment {
        additional_dependencies,
        commits,
        ..
    } = &exported
    else {
        panic!("expected fragment");
    };
    assert_eq!(additional_dependencies, &BTreeSet::from([*x.hash()]));
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
        assert!(relay.has_admitted_commit(meta.hash()));
    }
}
