//! Sedimentree API contracts. `contract_*` tests intentionally fail on todo!()
//! until the implementation lands. The codec fixture tests already run normally.
//! No MVR internals or mock verification/reconciliation implementation is used.

#[path = "sedimentree/codec.rs"]
mod codec;
mod support;

use support::{author, ingest_proof, key, proof, sign};

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
    sync::OnceLock,
};

use codec::{BatchCodec, Record, Write, encode_loose, encode_records};
use ed25519_dalek::SigningKey;
use stockfoil::{
    BlobHash, BlobMeta, Checkpoint, CommitContents, CommitGraph, CommitHash, CommitMeta,
    CommitSignature, EncodedItem, Fragment, FragmentRange, FragmentRef, ItemId, ItemMeta, ItemRef,
    LooseCommit, SedimentreeSummary, Side, SyncCodec, WritePermission, verify_item,
};

// Mine actual commit hashes rather than fabricating metadata or weakening the
// production depth metric. A depth-2 fixture takes roughly 65K trials, once.
fn record(key: &SigningKey, parents: &[CommitHash], depth: usize, value: &str) -> Record {
    for nonce in 0..2_000_000 {
        let payload = Write {
            key: "title".into(),
            value: value.into(),
            nonce,
        };
        let meta = CommitMeta::new(CommitContents {
            parents: parents.to_vec(),
            author: author(key),
            payload: BlobHash::of_contents(&bcs::to_bytes(&payload).unwrap()),
        });
        if meta
            .hash()
            .as_bytes()
            .iter()
            .take_while(|b| **b == 0)
            .count()
            == depth
        {
            return Record { meta, payload };
        }
    }
    panic!("fixture could not find a depth-{depth} commit");
}

fn hash(record: &Record) -> CommitHash {
    *record.meta.hash()
}

fn blob(bytes: &[u8]) -> BlobMeta {
    BlobMeta {
        hash: BlobHash::of_contents(bytes),
        size_bytes: bytes.len() as u64,
    }
}

fn checkpoint(hash: CommitHash) -> Checkpoint {
    Checkpoint(hash.as_bytes()[..12].try_into().unwrap())
}

fn loose(record: &Record, proof: CommitSignature) -> EncodedItem {
    let bytes = encode_loose(record, proof.signature);
    EncodedItem {
        metadata: ItemMeta::LooseCommit(LooseCommit {
            commit: hash(record),
            parents: record.meta.parents().iter().copied().collect(),
            blob: blob(&bytes),
        }),
        bytes,
    }
}

// These builders encode claims, not verified objects. Deliberately accept bad
// ranges, incomplete contents, and bad proofs so verify_item must check them.
fn fragment(
    range: FragmentRange,
    checkpoints: BTreeSet<Checkpoint>,
    records: &[Record],
    proofs: &[CommitSignature],
) -> EncodedItem {
    let bytes = encode_records(records, proofs);
    EncodedItem {
        metadata: ItemMeta::Fragment(Fragment {
            range,
            checkpoints,
            blob: blob(&bytes),
        }),
        bytes,
    }
}

fn encode_fragment(fragment: FragmentRef<'_, Write>, codec: &mut BatchCodec) -> EncodedItem {
    let range = fragment.range().clone();
    let checkpoints = fragment.checkpoints().clone();
    let bytes = codec.encode_fragment(fragment).unwrap();
    EncodedItem {
        metadata: ItemMeta::Fragment(Fragment {
            range,
            checkpoints,
            blob: blob(&bytes),
        }),
        bytes,
    }
}

fn item_ref(item: &EncodedItem) -> ItemRef {
    match &item.metadata {
        ItemMeta::LooseCommit(meta) => ItemRef {
            id: ItemId::Commit(meta.commit),
            blob: meta.blob,
        },
        ItemMeta::Fragment(meta) => ItemRef {
            id: ItemId::Fragment(meta.range.clone()),
            blob: meta.blob,
        },
    }
}

fn summary(items: &[EncodedItem]) -> SedimentreeSummary {
    let mut result = SedimentreeSummary::default();
    for item in items {
        match &item.metadata {
            ItemMeta::LooseCommit(meta) => result.loose_commits.push(meta.clone()),
            ItemMeta::Fragment(meta) => result.fragments.push(meta.clone()),
        }
    }
    result
}

fn delivered(graph: &CommitGraph<Write>) -> Vec<CommitHash> {
    let mut hashes = Vec::new();
    graph.traverse_delivered(|meta, _| {
        hashes.push(*meta.hash());
        ControlFlow::Continue(())
    });
    hashes
}

struct Nested {
    records: Vec<Record>,
    proofs: Vec<CommitSignature>,
    inner: FragmentRange,
    outer: FragmentRange,
    checkpoints: BTreeSet<Checkpoint>,
}

fn nested() -> &'static Nested {
    static FIXTURE: OnceLock<Nested> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let key = key(42);
        let root = record(&key, &[], 0, "root");
        let inner = record(&key, &[hash(&root)], 1, "inner");
        let outer = record(&key, &[hash(&inner)], 2, "outer");
        Nested {
            inner: FragmentRange {
                head: hash(&inner),
                boundary: BTreeSet::new(),
            },
            outer: FragmentRange {
                head: hash(&outer),
                boundary: BTreeSet::new(),
            },
            checkpoints: BTreeSet::from([checkpoint(hash(&inner))]),
            proofs: vec![proof(&key, hash(&inner)), proof(&key, hash(&outer))],
            records: vec![root, inner, outer],
        }
    })
}

#[test]
fn contract_loose_export_requires_direct_signatures_and_ignores_revocation() {
    let key = key(1);
    let root = record(&key, &[], 0, "root");
    let tip = record(&key, &[hash(&root)], 0, "tip");
    let mut graph = CommitGraph::new();
    graph.set_write_permissions(&HashMap::from([(author(&key), WritePermission::Unbounded)]));
    graph.ingest_local_commit(root.meta.clone(), root.payload.clone());
    graph.ingest_local_commit(tip.meta.clone(), tip.payload.clone());
    sign(&mut graph, &key, hash(&tip));
    // Global authentication is insufficient for exporting the loose ancestor.
    assert!(
        graph
            .resolve(&graph.signed_view_point())
            .unwrap()
            .visible(&hash(&root))
            .unwrap()
    );
    assert_eq!(graph.as_sedimentree().loose_commits().count(), 0);
    assert_eq!(graph.as_sedimentree().fragments().count(), 0);
    assert!(
        graph
            .as_sedimentree()
            .encode(&mut BatchCodec::default())
            .unwrap()
            .items()
            .is_empty()
    );

    sign(&mut graph, &key, hash(&root));
    let mut codec = BatchCodec::default();
    let batch = graph.as_sedimentree().encode(&mut codec).unwrap();
    assert_eq!((codec.loose_encodes, codec.fragment_encodes), (2, 0));
    assert_eq!(batch.items().len(), 2);
    let before = batch.summary();
    graph.set_write_permissions(&HashMap::new());
    let revoked = graph
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    assert_eq!(revoked.summary(), before);
    assert!(
        !graph
            .resolve(&graph.signed_view_point())
            .unwrap()
            .visible(&hash(&root))
            .unwrap()
    );
    for item in batch.items() {
        verify_item(item, &mut BatchCodec::default()).unwrap();
        assert_eq!(batch.get(&item_ref(item)), Some(item));
    }
    let draft = record(&key, &[hash(&tip)], 0, "unpublished");
    graph.ingest_local_commit(draft.meta.clone(), draft.payload.clone());
    let export = graph
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    assert_eq!(export.summary(), before);
    assert!(
        export
            .get(&item_ref(&loose(&draft, proof(&key, hash(&draft)))))
            .is_none()
    );
}

#[test]
fn contract_two_peers_exchange_wants_and_needs_without_permissions_or_duplicates() {
    let alice = key(1);
    let bob = key(2);
    let a = record(&alice, &[], 0, "alice");
    let b = record(&bob, &[], 0, "bob");
    let mut left = CommitGraph::new();
    let mut right = CommitGraph::new();
    for (graph, record, key) in [(&mut left, &a, &alice), (&mut right, &b, &bob)] {
        graph.set_write_permissions(&HashMap::from([(author(key), WritePermission::Unbounded)]));
        graph.ingest_local_commit(record.meta.clone(), record.payload.clone());
        sign(graph, key, hash(record));
    }
    let left_batch = left
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    let right_batch = right
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    let plan = left_batch
        .summary()
        .reconcile(&right_batch.summary())
        .unwrap();
    assert_eq!(
        plan.wants,
        vec![item_ref(&loose(&b, proof(&bob, hash(&b))))]
    );
    assert_eq!(
        plan.needs,
        vec![item_ref(&loose(&a, proof(&alice, hash(&a))))]
    );
    assert!(plan.fixups.is_empty());
    let incoming = right_batch.get(&plan.wants[0]).unwrap();
    let report = left.ingest_verified(verify_item(incoming, &mut BatchCodec::default()).unwrap());
    assert_eq!(report.delivered, vec![hash(&b)]);
    assert!(report.pending.is_empty());
    let duplicate =
        left.ingest_verified(verify_item(incoming, &mut BatchCodec::default()).unwrap());
    assert!(duplicate.delivered.is_empty());
    assert!(duplicate.pending.is_empty());
    let incoming = left_batch.get(&plan.needs[0]).unwrap();
    right.ingest_verified(verify_item(incoming, &mut BatchCodec::default()).unwrap());
    assert_eq!(delivered(&left).len(), 2);
    assert_eq!(delivered(&right).len(), 2);
    assert!(
        !left
            .resolve(&left.signed_view_point())
            .unwrap()
            .visible(&hash(&b))
            .unwrap()
    );
    assert!(
        !right
            .resolve(&right.signed_view_point())
            .unwrap()
            .visible(&hash(&a))
            .unwrap()
    );
    let left_now = left
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    let right_now = right
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    let done = left_now.summary().reconcile(&right_now.summary()).unwrap();
    assert!(done.wants.is_empty());
    assert!(done.needs.is_empty());
}

#[test]
fn contract_standalone_fragment_verification_precedes_causal_delivery() {
    let alice = key(1);
    let bob = key(2);
    let external = record(&alice, &[], 0, "external");
    let boundary = record(&alice, &[hash(&external)], 1, "boundary");
    let head = record(&bob, &[hash(&boundary)], 1, "head");
    let range = FragmentRange {
        head: hash(&head),
        boundary: BTreeSet::from([hash(&boundary)]),
    };
    let packet = fragment(
        range,
        BTreeSet::new(),
        &[boundary.clone(), head.clone()],
        &[proof(&alice, hash(&boundary)), proof(&bob, hash(&head))],
    );
    // No graph, external parent, or cached signature is supplied to verification.
    let verified = verify_item(&packet, &mut BatchCodec::default()).unwrap();
    let mut graph = CommitGraph::new();
    graph.set_write_permissions(&HashMap::from([
        (author(&alice), WritePermission::Unbounded),
        (author(&bob), WritePermission::Unbounded),
    ]));
    let before = graph.view_point();
    let report = graph.ingest_verified(verified);
    assert!(report.delivered.is_empty());
    assert_eq!(
        report.pending.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([hash(&boundary), hash(&head)])
    );
    assert!(delivered(&graph).is_empty());
    let parent = loose(&external, proof(&alice, hash(&external)));
    let report = graph.ingest_verified(verify_item(&parent, &mut BatchCodec::default()).unwrap());
    assert_eq!(
        report.delivered,
        vec![hash(&external), hash(&boundary), hash(&head)]
    );
    assert!(report.pending.is_empty());
    let current = graph.resolve(&graph.signed_view_point()).unwrap();
    for record in [&external, &boundary, &head] {
        assert!(current.visible(&hash(record)).unwrap());
        assert!(
            !graph
                .resolve(&before)
                .unwrap()
                .visible(&hash(record))
                .unwrap()
        );
    }
    let duplicate =
        graph.ingest_verified(verify_item(&packet, &mut BatchCodec::default()).unwrap());
    assert!(duplicate.delivered.is_empty());
    assert!(duplicate.pending.is_empty());
}

#[test]
fn contract_persisted_batch_retains_proofs_for_reexporting_smaller_fragments() {
    let fixture = nested();
    let packet = fragment(
        fixture.outer.clone(),
        fixture.checkpoints.clone(),
        &fixture.records,
        &fixture.proofs,
    );
    let mut graph = CommitGraph::new();
    graph.ingest_verified(verify_item(&packet, &mut BatchCodec::default()).unwrap());
    // This replica has received bytes, not a signing key or individual old blobs.
    let mut codec = BatchCodec::default();
    let view = graph.as_sedimentree();
    assert_eq!(view.loose_commits().count(), 0);
    assert_eq!(view.fragments().count(), 1);
    let persisted = view.encode(&mut codec).unwrap();
    assert_eq!((codec.loose_encodes, codec.fragment_encodes), (0, 1));
    assert_eq!(persisted.items().len(), 1);
    let mut restored = CommitGraph::new();
    restored
        .ingest_verified(verify_item(&persisted.items()[0], &mut BatchCodec::default()).unwrap());
    let restored_view = restored.as_sedimentree();
    let inner = restored_view
        .fragment(&fixture.inner)
        .expect("canonical child survives compaction");
    let retained: BTreeSet<_> = inner.signatures().map(|s| (s.author, s.commit)).collect();
    assert!(retained.contains(&(fixture.proofs[0].author, fixture.inner.head)));
    // The full batch can be traversed repeatedly without consuming the view.
    assert_eq!(
        inner.commits().map(|c| *c.meta.hash()).collect::<Vec<_>>(),
        vec![hash(&fixture.records[0]), fixture.inner.head]
    );
    assert_eq!(inner.commits().count(), 2);
    let encoded_inner = encode_fragment(inner, &mut codec);
    assert_eq!((codec.loose_encodes, codec.fragment_encodes), (0, 2));
    let mut empty = CommitGraph::new();
    let report =
        empty.ingest_verified(verify_item(&encoded_inner, &mut BatchCodec::default()).unwrap());
    assert_eq!(
        report.delivered,
        vec![hash(&fixture.records[0]), fixture.inner.head]
    );
    assert!(report.pending.is_empty());
}

#[test]
fn contract_global_authentication_cannot_replace_a_missing_nested_fragment_proof() {
    let fixture = nested();
    let mut graph = CommitGraph::new();
    graph.set_write_permissions(&HashMap::from([(
        fixture.proofs[1].author,
        WritePermission::Unbounded,
    )]));
    for record in &fixture.records {
        graph.ingest_commit(record.meta.clone(), record.payload.clone());
    }
    ingest_proof(&mut graph, fixture.proofs[1]);
    // The existing graph can authenticate every commit from the outer signature.
    let resolved = graph.resolve(&graph.signed_view_point()).unwrap();
    assert!(
        fixture
            .records
            .iter()
            .all(|r| resolved.visible(&hash(r)).unwrap())
    );
    let incomplete = fragment(
        fixture.outer.clone(),
        fixture.checkpoints.clone(),
        &fixture.records,
        &fixture.proofs[1..],
    );
    // But reconstructing the inner range would need a signature outside it.
    assert!(verify_item(&incomplete, &mut BatchCodec::default()).is_err());
}

#[test]
fn contract_concurrent_writes_by_one_author_need_coverage_on_both_branches() {
    let alice = key(1);
    let bob = key(2);
    let root = record(&alice, &[], 0, "root");
    let left = record(&alice, &[hash(&root)], 0, "left");
    let right = record(&alice, &[hash(&root)], 0, "right");
    let head = record(&bob, &[hash(&left), hash(&right)], 1, "merge");
    let range = FragmentRange {
        head: hash(&head),
        boundary: BTreeSet::new(),
    };
    let proofs = [
        proof(&alice, hash(&left)),
        proof(&alice, hash(&right)),
        proof(&bob, hash(&head)),
    ];
    let records = [root, left, right, head];
    let complete = fragment(range.clone(), BTreeSet::new(), &records, &proofs);
    verify_item(&complete, &mut BatchCodec::default()).unwrap();
    // Alice's left signature proves authorship of the shared root, but not the
    // concurrent right write. Bob's merge signature cannot fill that gap.
    let missing_branch = fragment(range, BTreeSet::new(), &records, &[proofs[0], proofs[2]]);
    assert!(verify_item(&missing_branch, &mut BatchCodec::default()).is_err());
}

#[test]
fn contract_invalid_items_are_rejected_without_any_graph_mutation() {
    let alice = key(1);
    let bob = key(2);
    let root = record(&alice, &[], 0, "root");
    let head = record(&bob, &[hash(&root)], 1, "head");
    let outside = record(&bob, &[hash(&head)], 0, "outside");
    let unrelated = record(&alice, &[], 0, "unrelated");
    let range = FragmentRange {
        head: hash(&head),
        boundary: BTreeSet::new(),
    };
    let records = vec![root.clone(), head.clone()];
    let proofs = vec![proof(&alice, hash(&root)), proof(&bob, hash(&head))];
    let valid = fragment(range.clone(), BTreeSet::new(), &records, &proofs);
    let mut corrupt_bytes = valid.clone();
    *corrupt_bytes.bytes.last_mut().unwrap() ^= 1; // Keep the original blob hash.
    let mut wrong_size = valid.clone();
    if let ItemMeta::Fragment(meta) = &mut wrong_size.metadata {
        meta.blob.size_bytes += 1;
    }
    let mut substituted = records.clone();
    substituted[0].payload.value = "substituted".into(); // Rehash the blob, not the payload commitment.
    let mut wrong_signature = proofs.clone();
    wrong_signature[1].signature = proof(&bob, hash(&root)).signature;
    let cases = vec![
        ("blob hash", corrupt_bytes),
        ("blob size", wrong_size),
        (
            "payload commitment",
            fragment(range.clone(), BTreeSet::new(), &substituted, &proofs),
        ),
        (
            "signature for another hash",
            fragment(range.clone(), BTreeSet::new(), &records, &wrong_signature),
        ),
        (
            "missing author",
            fragment(range.clone(), BTreeSet::new(), &records, &proofs[1..]),
        ),
        (
            "wrong author",
            fragment(
                range.clone(),
                BTreeSet::new(),
                &records,
                &[proof(&bob, hash(&root)), proofs[1]],
            ),
        ),
        (
            "outside proof target",
            fragment(
                range.clone(),
                BTreeSet::new(),
                &records,
                &[proofs[0], proof(&bob, hash(&outside))],
            ),
        ),
        (
            "missing interior commit",
            fragment(range.clone(), BTreeSet::new(), &records[1..], &proofs[1..]),
        ),
        (
            "unrelated extra commit",
            fragment(
                range.clone(),
                BTreeSet::new(),
                &[root.clone(), unrelated.clone(), head.clone()],
                &[proofs[0], proofs[1], proof(&alice, hash(&unrelated))],
            ),
        ),
        (
            "invalid checkpoint",
            fragment(
                range.clone(),
                BTreeSet::from([checkpoint(hash(&outside))]),
                &records,
                &proofs,
            ),
        ),
        (
            "invalid boundary depth",
            fragment(
                FragmentRange {
                    head: hash(&head),
                    boundary: BTreeSet::from([hash(&root)]),
                },
                BTreeSet::new(),
                &records,
                &proofs,
            ),
        ),
        (
            "missing fragment head",
            fragment(range, BTreeSet::new(), &[], &[]),
        ),
    ];
    let mut graph = CommitGraph::new();
    graph.ingest_verified(verify_item(&valid, &mut BatchCodec::default()).unwrap());
    let before = graph.view_point();
    for (label, item) in cases {
        // Even this populated graph cannot supply missing/outside evidence.
        assert!(
            verify_item(&item, &mut BatchCodec::default()).is_err(),
            "{label}"
        );
        assert_eq!(graph.view_point(), before);
    }
}

#[test]
fn contract_reconciliation_prefers_covering_fragments_and_reports_conditional_fixups() {
    let key = key(1);
    let root = record(&key, &[], 0, "root");
    let head = record(&key, &[hash(&root)], 1, "head");
    let root_proof = proof(&key, hash(&root));
    let head_proof = proof(&key, hash(&head));
    let range = FragmentRange {
        head: hash(&head),
        boundary: BTreeSet::new(),
    };
    let loose_items = [loose(&root, root_proof), loose(&head, head_proof)];
    let packet = fragment(
        range.clone(),
        BTreeSet::new(),
        &[root.clone(), head.clone()],
        &[root_proof, head_proof],
    );
    let local = summary(&loose_items);
    let remote = summary(std::slice::from_ref(&packet));
    let local_before = local.clone();
    let remote_before = remote.clone();
    let plan = local.reconcile(&remote).unwrap();
    assert_eq!(plan.wants, vec![item_ref(&packet)]);
    assert!(plan.needs.is_empty());
    assert_eq!(plan.fixups.len(), 2);
    assert_eq!(
        plan.fixups
            .iter()
            .map(|f| f.redundant.clone())
            .collect::<BTreeSet<_>>(),
        loose_items.iter().map(item_ref).collect()
    );
    for fixup in &plan.fixups {
        assert_eq!(fixup.side, Side::Local);
        assert_eq!(fixup.covered_by, vec![item_ref(&packet)]);
    }
    assert_eq!(local, local_before);
    assert_eq!(remote, remote_before);
    let reverse = remote.reconcile(&local).unwrap();
    assert!(reverse.wants.is_empty());
    assert_eq!(reverse.needs, vec![item_ref(&packet)]);
    assert_eq!(reverse.fixups.len(), 2);
    assert!(reverse.fixups.iter().all(|f| f.side == Side::Remote));

    // Both encodings contain the same valid evidence, in different wire order.
    let alternate = fragment(
        range,
        BTreeSet::new(),
        &[root, head],
        &[head_proof, root_proof],
    );
    assert_ne!(alternate.bytes, packet.bytes);
    assert_ne!(item_ref(&alternate).blob, item_ref(&packet).blob);
    let equivalent = remote.reconcile(&summary(&[alternate])).unwrap();
    assert!(equivalent.wants.is_empty());
    assert!(equivalent.needs.is_empty());
}

#[test]
fn contract_merge_ranges_and_encoded_cover_do_not_depend_on_arrival_order() {
    let alice = key(1);
    let bob = key(2);
    let root = record(&alice, &[], 0, "root");
    let left = record(&alice, &[hash(&root)], 1, "left");
    let right = record(&bob, &[hash(&root)], 1, "right");
    let head = record(&alice, &[hash(&left), hash(&right)], 2, "merge");
    let proofs = [
        proof(&alice, hash(&root)),
        proof(&alice, hash(&left)),
        proof(&bob, hash(&right)),
        proof(&alice, hash(&head)),
    ];
    let records = [root, left, right, head];
    let mut forward = CommitGraph::new();
    let mut reverse = CommitGraph::new();
    for record in &records {
        forward.ingest_commit(record.meta.clone(), record.payload.clone());
    }
    for record in records.iter().rev() {
        reverse.ingest_commit(record.meta.clone(), record.payload.clone());
    }
    for proof in &proofs {
        ingest_proof(&mut forward, *proof);
    }
    for proof in proofs.iter().rev() {
        ingest_proof(&mut reverse, *proof);
    }
    let mut codec = BatchCodec::default();
    let left_batch = forward.as_sedimentree().encode(&mut codec).unwrap();
    let right_batch = reverse
        .as_sedimentree()
        .encode(&mut BatchCodec::default())
        .unwrap();
    assert_eq!(left_batch.summary(), right_batch.summary());
    assert_eq!((codec.loose_encodes, codec.fragment_encodes), (0, 1));
    assert_eq!(left_batch.items().len(), 1);
    let meta = match &left_batch.items()[0].metadata {
        ItemMeta::Fragment(meta) => meta,
        _ => panic!("merge should form a covering fragment"),
    };
    assert_eq!(meta.range.head, hash(&records[3]));
    assert!(meta.range.boundary.is_empty());
    assert_eq!(
        meta.checkpoints,
        BTreeSet::from([checkpoint(hash(&records[1])), checkpoint(hash(&records[2]))])
    );
    verify_item(&left_batch.items()[0], &mut BatchCodec::default()).unwrap();
}
