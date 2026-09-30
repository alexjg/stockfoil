//! Decoder streams, per-author signature boundaries, and signed head announcements.
use std::collections::BTreeSet;

use super::support;

use super::CommitGraph;
use crate::{
    Author, BlobHash, Checkpoint, CommitContents, CommitHash, CommitMeta, ExportError,
    FragmentRange, FragmentReadError, ItemId, UnverifiedItem, VerificationError,
};
use support::{
    at_depth as meta, proof as sig, seeded_author as author, signed_commit as commit_item,
    signed_fragment,
};

fn root_range(head: &CommitMeta) -> FragmentRange {
    FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    }
}

#[test]
fn a_decoder_reconstructs_omitted_hashes_and_parent_references_as_verification_pulls() {
    let root = meta(1, vec![], 0);
    let middle = meta(2, vec![*root.hash()], 0);
    let head = meta(1, vec![*middle.hash(), *root.hash(), *root.hash()], 1);
    // The application format contains parent offsets, authors and payload hashes,
    // but no commit hashes. Real applications can decompress payloads separately.
    let encoded = [
        (*root.author(), vec![], *root.payload_hash()),
        (*middle.author(), vec![0], *middle.payload_hash()),
        (*head.author(), vec![1, 0, 0], *head.payload_hash()),
    ];
    let mut decoded_hashes: Vec<CommitHash> = Vec::new();
    let mut calls = 0;
    let decoder = encoded.into_iter().map(|(author, references, payload)| {
        calls += 1;
        let parents = references
            .into_iter()
            .map(|index| {
                decoded_hashes
                    .get(index)
                    .copied()
                    .ok_or(VerificationError::InvalidItem)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let envelope = CommitMeta::new(CommitContents {
            author,
            parents,
            payload,
        });
        decoded_hashes.push(*envelope.hash());
        Ok(envelope)
    });
    let verified = UnverifiedItem::verify_fragment(
        root_range(&head),
        vec![sig(1, *head.hash()), sig(2, *middle.hash())],
        decoder,
    )
    .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(decoded_hashes, [*root.hash(), *middle.hash(), *head.hash()]);
    assert_eq!(
        verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
        decoded_hashes
    );
    let mut graph = CommitGraph::new(Default::default());
    graph.ingest_verified(verified, |meta| *meta.payload_hash()).unwrap();
    graph
        .export_item(&ItemId::Fragment(root_range(&head)))
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn decoder_failure_stops_consumption_without_producing_a_certificate() {
    let root = meta(1, vec![], 0);
    let head = meta(1, vec![*root.hash()], 1);
    let mut calls = 0;
    let decoder = std::iter::from_fn(|| {
        calls += 1;
        Some(match calls {
            1 => Ok(root.clone()),
            2 => Err(VerificationError::InvalidItem),
            _ => panic!("must stop at the malformed compressed record"),
        })
    });
    assert_eq!(
        UnverifiedItem::verify_fragment(root_range(&head), vec![sig(1, *head.hash())], decoder)
            .unwrap_err(),
        VerificationError::InvalidItem
    );
    assert_eq!(calls, 2);
}

#[test]
fn signature_boundary_must_dominate_every_concurrent_latest_authored_commit() {
    let root = meta(1, vec![], 0);
    let left = meta(1, vec![*root.hash()], 0);
    let right = meta(1, vec![*root.hash(), *root.hash()], 0);
    let head = meta(2, vec![*left.hash(), *right.hash()], 1);
    let range = root_range(&head);
    let members = vec![head.clone(), right.clone(), root.clone(), left.clone()];
    let verify = |signatures| {
        UnverifiedItem::Fragment {
            range: range.clone(),
            checkpoints: BTreeSet::new(),
            commits: members.clone(),
            signatures,
        }
        .verify()
    };
    let err = verify(vec![sig(1, *root.hash()), sig(2, *head.hash())]).unwrap_err();
    assert!(
        matches!(err, VerificationError::MissingSignature { author: a, commit }
        if a == author(1) && [*left.hash(), *right.hash()].contains(&commit))
    );
    assert_eq!(
        verify(vec![sig(1, *left.hash()), sig(2, *head.hash())]).unwrap_err(),
        VerificationError::MissingSignature {
            author: author(1),
            commit: *right.hash()
        }
    );

    let verified = verify(vec![
        sig(1, *root.hash()),
        sig(1, *left.hash()),
        sig(1, *right.hash()),
        sig(2, *head.hash()),
    ])
    .unwrap();
    assert_eq!(
        verified.signature_boundary()[&author(1)],
        BTreeSet::from([*left.hash(), *right.hash()])
    );
    assert_eq!(
        verified.signature_boundary()[&author(2)],
        BTreeSet::from([*head.hash()])
    );
    let mut graph = CommitGraph::new(Default::default());
    graph.ingest_verified(verified, |_| ()).unwrap();
    let UnverifiedItem::Fragment { signatures, .. } =
        graph.export_item(&ItemId::Fragment(range.clone())).unwrap()
    else {
        panic!("expected a fragment");
    };
    // The root signature was checked but is not part of the signature boundary.
    assert_eq!(signatures.len(), 3);
    assert!(
        !signatures
            .iter()
            .any(|evidence| evidence.commit == *root.hash())
    );

    // An author can instead sign a later member authored by somebody else.
    let co_signed = verify(vec![sig(1, *head.hash()), sig(2, *head.hash())]).unwrap();
    assert_eq!(
        co_signed.signature_boundary()[&author(1)],
        BTreeSet::from([*head.hash()])
    );
}

#[test]
fn standalone_checkpoints_and_head_announcements_do_not_invent_fragment_certificates() {
    let root = meta(1, vec![], 0);
    let boundary = meta(2, vec![*root.hash()], 1);
    let middle = meta(2, vec![*boundary.hash()], 1);
    let head = meta(3, vec![*middle.hash()], 2);
    let mut source = CommitGraph::new(Default::default());
    source
        .ingest_verified(commit_item(&root, 1).verify().unwrap(), |_| 1)
        .unwrap();
    source
        .ingest_verified(commit_item(&boundary, 2).verify().unwrap(), |_| 2)
        .unwrap();
    assert_eq!(source.minimal_cover().loose_commits.len(), 2);
    assert!(source.minimal_cover().fragments.is_empty());
    assert_eq!(
        source
            .export_item(&ItemId::Fragment(root_range(&boundary)))
            .unwrap_err(),
        ExportError::NotInMinimalCover(ItemId::Fragment(root_range(&boundary)))
    );
    assert_eq!(
        source.traverse_fragment(&root_range(&boundary), |_, _| {
            std::ops::ControlFlow::Continue(())
        }),
        Err(FragmentReadError::NotFragment(*boundary.hash()))
    );

    // A bare checkpoint can be a boundary even though its fragment is unknown.
    let middle_range = FragmentRange {
        head: *middle.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    source
        .ingest_verified(
            signed_fragment(&middle_range, vec![middle.clone()], 2)
                .verify()
                .unwrap(),
            |_| 3,
        )
        .unwrap();
    assert_eq!(source.minimal_cover().fragments[0].range, middle_range);
    let mut receiver = CommitGraph::new(Default::default());
    receiver.merge_fixture(&source);
    assert_eq!(receiver.minimal_cover(), source.minimal_cover());

    // A larger certified range indexes the previously bare checkpoint as an
    // interior constituent. Its signature boundary refers to the real targets.
    let range = root_range(&head);
    source
        .ingest_verified(
            UnverifiedItem::Fragment {
                range: range.clone(),
                checkpoints: BTreeSet::new(),
                commits: vec![head.clone(), middle.clone(), boundary.clone(), root.clone()],
                signatures: vec![
                    sig(3, *head.hash()),
                    sig(2, *middle.hash()),
                    sig(1, *root.hash()),
                ],
            }
            .verify()
            .unwrap(),
            |_| 4,
        )
        .unwrap();
    // The former bare checkpoint was indexed after `middle`, even though it
    // precedes it in the commit array. Both remain in the non-minimal summary.
    let summary = source.as_sedimentree().summary();
    assert_eq!(
        summary
            .fragments
            .iter()
            .map(|f| f.range.head)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*boundary.hash(), *middle.hash(), *head.hash()])
    );
    let cover = source.minimal_cover();
    assert!(cover.loose_commits.is_empty());
    assert_eq!(cover.fragments.len(), 1);
    assert_eq!(cover.fragments[0].range, range);
    assert_eq!(
        cover.fragments[0].checkpoints,
        [&boundary, &middle]
            .map(|m| Checkpoint(m.hash().as_bytes()[..12].try_into().unwrap()))
            .into()
    );
    assert!(source.export_item(&ItemId::Commit(*boundary.hash())).is_err());

    let announcement = source
        .export_item(&ItemId::Commit(*head.hash()))
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(announcement.commits().count(), 1);
    assert_eq!(
        announcement.dependencies().copied().collect::<Vec<_>>(),
        vec![*middle.hash()]
    );
    receiver.ingest_verified(announcement, |_| 40).unwrap();
    // Learning the signed head reveals its link to the previous chain, but does
    // not claim to have the larger fragment's per-author certificate yet.
    let announced = receiver.minimal_cover();
    assert_eq!(announced.fragments.len(), 1);
    assert_eq!(announced.fragments[0].range, middle_range);
    assert_eq!(
        announced
            .loose_commits
            .iter()
            .map(|c| c.commit)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*root.hash(), *boundary.hash(), *head.hash()])
    );
    assert!(receiver.export_item(&ItemId::Fragment(range.clone())).is_err());
    receiver
        .ingest_verified(
            source
                .export_item(&ItemId::Fragment(range.clone()))
                .unwrap()
                .verify()
                .unwrap(),
            |_| panic!("already have all payloads"),
        )
        .unwrap();
    assert_eq!(*receiver.publishable_commit(head.hash()).unwrap().1, 40);
    assert_eq!(receiver.minimal_cover(), source.minimal_cover());
    let mut relay = CommitGraph::new(Default::default());
    relay.merge_fixture(&receiver);
    assert_eq!(relay.minimal_cover(), source.minimal_cover());
    relay
        .export_item(&ItemId::Fragment(range))
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn seeded_dags_match_an_independent_author_frontier_oracle() {
    use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
    use std::collections::BTreeMap;

    const SEED: u64 = 0xa017_40f2;
    const N: usize = 16;
    let mut rng = StdRng::seed_from_u64(SEED);
    for case in 0..12 {
        let mut members: Vec<CommitMeta> = Vec::new();
        let mut authors = Vec::new();
        let mut reachable = [[false; N]; N];
        for index in 0..N {
            let seed = rng.random_range(1..=3);
            let parents: Vec<usize> = if index == N - 1 {
                (0..index).collect() // Every generated member is in the fragment.
            } else if index == 0 {
                vec![]
            } else {
                (0..rng.random_range(0..=3))
                    .map(|_| rng.random_range(0..index))
                    .collect()
            };
            for &parent in &parents {
                reachable[index][parent] = true;
            }
            let signer = author(seed);
            let envelope = (0u64..1_000_000)
                .map(|nonce| {
                    CommitMeta::new(CommitContents {
                        author: signer,
                        parents: parents.iter().map(|&i| *members[i].hash()).collect(),
                        payload: BlobHash::of_contents(
                            &[index as u64, nonce].map(u64::to_le_bytes).concat(),
                        ),
                    })
                })
                .find(|m| {
                    m.hash().as_bytes().iter().take_while(|&&b| b == 0).count()
                        == usize::from(index == N - 1)
                })
                .unwrap();
            authors.push(seed);
            members.push(envelope);
        }
        // Independent all-pairs transitive closure over generated integer edges,
        // not the verifier's hash traversal or frontier helper.
        for via in 0..N {
            for child in 0..N {
                for ancestor in 0..N {
                    reachable[child][ancestor] |= reachable[child][via] && reachable[via][ancestor];
                }
            }
        }
        let mut expected: BTreeMap<Author, BTreeSet<CommitHash>> = BTreeMap::new();
        for index in 0..N {
            if !(0..N).any(|other| authors[other] == authors[index] && reachable[other][index]) {
                expected
                    .entry(author(authors[index]))
                    .or_default()
                    .insert(*members[index].hash());
            }
        }
        let range = root_range(&members[N - 1]);
        let signatures: Vec<_> = members
            .iter()
            .zip(&authors)
            .map(|(m, &a)| sig(a, *m.hash()))
            .collect();
        members.shuffle(&mut rng);
        let verified = UnverifiedItem::verify_fragment(
            range.clone(),
            signatures.clone(),
            members.clone().into_iter().map(Ok),
        )
        .unwrap();
        assert_eq!(
            verified.signature_boundary(),
            &expected,
            "seed={SEED:#x} case={case}"
        );

        // All ancestor signatures remain, but cannot substitute for a missing
        // latest tip, including when that author has other concurrent tips.
        let missing = signatures
            .iter()
            .find(|s| expected[&s.author].contains(&s.commit))
            .unwrap();
        let incomplete = signatures
            .iter()
            .filter(|s| (s.author, s.commit) != (missing.author, missing.commit))
            .cloned()
            .collect();
        assert_eq!(
            UnverifiedItem::verify_fragment(range.clone(), incomplete, members.into_iter().map(Ok))
                .unwrap_err(),
            VerificationError::MissingSignature {
                author: missing.author,
                commit: missing.commit
            },
            "seed={SEED:#x} case={case}"
        );

        let mut graph = CommitGraph::new(Default::default());
        graph.ingest_verified(verified, |_| ()).unwrap();
        let relayed = graph
            .export_item(&ItemId::Fragment(range))
            .unwrap()
            .verify()
            .unwrap();
        assert_eq!(
            relayed.signature_boundary(),
            &expected,
            "seed={SEED:#x} case={case}"
        );
    }
}
