//! Adversarial contracts for stateless, item-local metadata authentication.
use std::collections::BTreeSet;

use crate::{
    BlobHash, CommitContents, CommitHash, CommitMeta, FragmentRange, ItemId, Signature,
    SignatureEvidence, UnverifiedItem, VerificationError,
};
use super::support::{
    at_depth, proof as evidence, seeded_author as author, signed_commit as signed,
    signed_fragment as fragment,
};

#[test]
fn missing_wrong_author_and_invalid_signatures_are_distinct() {
    let root = at_depth(1, vec![], 0);
    let missing = VerificationError::MissingSignature {
        author: author(1),
        commit: *root.hash(),
    };
    assert_eq!(
        UnverifiedItem::Commit {
            meta: root.clone(),
            signatures: vec![],
        }
        .verify()
        .unwrap_err(),
        missing
    );
    assert_eq!(signed(&root, 2).verify().unwrap_err(), missing);

    let mut bad = evidence(1, *root.hash());
    bad.signature = Signature::from([0; 64]);
    let extra_invalid = UnverifiedItem::Commit {
        meta: root.clone(),
        signatures: vec![evidence(1, *root.hash()), bad],
    };
    assert!(matches!(
        extra_invalid.verify(),
        Err(VerificationError::InvalidSignature(_))
    ));

    let wrong_message = UnverifiedItem::Commit {
        meta: root.clone(),
        signatures: vec![SignatureEvidence {
            commit: *root.hash(),
            ..evidence(1, CommitHash::from([99; 32]))
        }],
    };
    assert!(matches!(
        wrong_message.verify(),
        Err(VerificationError::InvalidSignature(_))
    ));
}

#[test]
fn author_payload_parent_order_and_multiplicity_are_hash_bound() {
    let left = at_depth(1, vec![], 0);
    let right = at_depth(2, vec![], 0);
    let original = at_depth(1, vec![*left.hash(), *right.hash()], 0);
    let contents = CommitContents {
        parents: original.parents().to_vec(),
        author: *original.author(),
        payload: *original.payload_hash(),
    };
    assert_eq!(CommitMeta::new(contents.clone()).hash(), original.hash());
    for changed in [
        CommitContents {
            author: author(2),
            ..contents.clone()
        },
        CommitContents {
            payload: BlobHash::of_contents(b"tampered"),
            ..contents.clone()
        },
        CommitContents {
            parents: vec![*right.hash(), *left.hash()],
            ..contents.clone()
        },
        CommitContents {
            parents: vec![*left.hash(), *right.hash(), *left.hash()],
            ..contents
        },
    ] {
        let changed = CommitMeta::new(changed);
        assert_ne!(changed.hash(), original.hash());
        // Retargeting the original signature cannot authenticate changed bytes.
        let result = UnverifiedItem::Commit {
            meta: changed.clone(),
            signatures: vec![SignatureEvidence {
                commit: *changed.hash(),
                ..evidence(1, *original.hash())
            }],
        }
        .verify();
        assert!(matches!(
            result,
            Err(VerificationError::InvalidSignature(_))
        ));
    }
}

#[test]
fn dependencies_are_sorted_distinct_hashes_not_verification_prerequisites() {
    let left = at_depth(1, vec![], 0);
    let right = at_depth(2, vec![], 0);
    let parents = vec![*right.hash(), *left.hash(), *right.hash()];
    let child = at_depth(1, parents.clone(), 0);
    let incoming = signed(&child, 1);
    let expected: Vec<_> = BTreeSet::from([*left.hash(), *right.hash()])
        .into_iter()
        .collect();
    assert_eq!(incoming.id(), ItemId::Commit(*child.hash()));
    assert_eq!(incoming.dependencies().collect::<Vec<_>>(), expected);
    // Neither parent has been verified or admitted anywhere.
    let verified = incoming.verify().unwrap();
    assert_eq!(verified.id(), &ItemId::Commit(*child.hash()));
    assert_eq!(
        verified.external_parents().copied().collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        expected
    );
    let members = verified.commits().collect::<Vec<_>>();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].hash(), child.hash());
    assert_eq!(members[0].parents(), parents);
}

#[test]
fn signature_targets_must_be_actual_members_even_when_evidence_is_redundant() {
    let parent = at_depth(1, vec![], 0);
    let child = at_depth(1, vec![*parent.hash()], 0);
    let later = at_depth(1, vec![*child.hash()], 0);
    let unrelated = at_depth(2, vec![], 0);
    for target in [
        *parent.hash(),
        *later.hash(),
        *unrelated.hash(),
        CommitHash::from([99; 32]),
    ] {
        for redundant in [false, true] {
            let mut signatures = vec![evidence(1, target)];
            if redundant {
                signatures.insert(0, evidence(1, *child.hash()));
            }
            assert_eq!(
                UnverifiedItem::Commit {
                    meta: child.clone(),
                    signatures,
                }
                .verify()
                .unwrap_err(),
                VerificationError::InvalidItem
            );
        }
    }
}

#[test]
fn standalone_commit_and_fragment_certificates_are_distinct_at_nonzero_depth() {
    let head = at_depth(1, vec![], 1);
    let commit = signed(&head, 1).verify().unwrap();
    assert_eq!(commit.id(), &ItemId::Commit(*head.hash()));
    assert_eq!(commit.external_parents().count(), 0);
    assert_eq!(commit.dependencies().count(), 0);
    assert_eq!(
        commit.signature_boundary()[&author(1)],
        BTreeSet::from([*head.hash()])
    );
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let verified = fragment(&range, vec![head.clone()], 1).verify().unwrap();
    assert_eq!(verified.id(), &ItemId::Fragment(range));
    assert_eq!(verified.dependencies().count(), 0);
    assert_eq!(verified.external_parents().count(), 0);
    assert_eq!(verified.commits().next().unwrap().hash(), head.hash());
}

#[test]
fn fragment_members_must_be_reachable_and_input_order_does_not_matter() {
    let root = at_depth(1, vec![], 0);
    let middle = at_depth(1, vec![*root.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let extra = at_depth(2, vec![], 0);
    for records in [
        vec![],
        vec![head.clone(), root.clone()],
        vec![middle.clone(), root.clone()],
        vec![root.clone(), middle.clone(), head.clone(), extra],
        vec![root.clone(), middle.clone(), head.clone(), root.clone()],
    ] {
        assert_eq!(
            fragment(&range, records, 1).verify().unwrap_err(),
            VerificationError::InvalidItem
        );
    }
    for records in [
        vec![root.clone(), middle.clone(), head.clone()],
        vec![head.clone(), root.clone(), middle.clone()],
        vec![middle.clone(), head.clone(), root.clone()],
    ] {
        let incoming = fragment(&range, records.clone(), 1);
        assert_eq!(incoming.dependencies().count(), 0);
        let streamed = UnverifiedItem::verify_fragment(
            range.clone(),
            vec![evidence(1, *head.hash())],
            records.into_iter().map(Ok),
        )
        .unwrap();
        let owned = incoming.verify().unwrap();
        for verified in [&owned, &streamed] {
            assert_eq!(verified.id(), &ItemId::Fragment(range.clone()));
            assert_eq!(verified.external_parents().count(), 0);
            assert_eq!(verified.dependencies().count(), 0);
            assert_eq!(
                verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
                vec![*root.hash(), *middle.hash(), *head.hash()]
            );
        }
        assert_eq!(owned.signature_boundary(), streamed.signature_boundary());
    }
    let zero_range = FragmentRange {
        head: *root.hash(),
        boundary: BTreeSet::new(),
    };
    assert_eq!(
        fragment(&zero_range, vec![root], 1).verify().unwrap_err(),
        VerificationError::InvalidItem
    );
}

#[test]
fn boundaries_are_exclusive_assumptions_without_external_parent_metadata() {
    let external = at_depth(2, vec![], 0);
    let boundary = at_depth(2, vec![*external.hash()], 1);
    let head = at_depth(1, vec![*boundary.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    let incoming = fragment(&range, vec![head.clone()], 1);
    assert_eq!(
        incoming.dependencies().collect::<Vec<_>>(),
        vec![*boundary.hash()]
    );
    // No boundary metadata, evidence for its author, or metadata for its parent.
    let verified = incoming.verify().unwrap();
    assert!(!verified.signature_boundary().contains_key(&author(2)));
    assert_eq!(
        verified.external_parents().copied().collect::<Vec<_>>(),
        vec![*boundary.hash()]
    );
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        vec![*boundary.hash()]
    );
    assert_eq!(
        verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
        vec![*head.hash()]
    );
    for records in [
        vec![head.clone(), boundary.clone()],
        vec![head.clone(), boundary.clone(), external],
    ] {
        assert_eq!(
            fragment(&range, records, 1).verify().unwrap_err(),
            VerificationError::InvalidItem
        );
    }
    // Dependencies are not valid signature targets, even with valid evidence.
    assert_eq!(
        UnverifiedItem::Fragment {
            range: range.clone(),
            checkpoints: BTreeSet::new(),
            commits: vec![head.clone()],
            signatures: vec![evidence(1, *head.hash()), evidence(2, *boundary.hash())],
        }
        .verify()
        .unwrap_err(),
        VerificationError::InvalidItem
    );
    for boundary_set in [BTreeSet::new(), BTreeSet::from([*head.hash()])] {
        let wrong_range = FragmentRange {
            head: *head.hash(),
            boundary: boundary_set,
        };
        assert_eq!(
            fragment(&wrong_range, vec![head.clone()], 1)
                .verify()
                .unwrap_err(),
            VerificationError::InvalidItem
        );
    }
}

#[test]
fn external_parents_are_derived_exactly_after_collecting_all_members() {
    let external = at_depth(2, vec![], 0);
    let boundary = at_depth(2, vec![*external.hash()], 1);
    let middle = at_depth(
        1,
        vec![*boundary.hash(), *external.hash(), *external.hash()],
        0,
    );
    let head = at_depth(1, vec![*middle.hash(), *middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    let expected: Vec<_> = BTreeSet::from([*boundary.hash(), *external.hash()])
        .into_iter()
        .collect();
    for commits in [
        vec![head.clone(), middle.clone()], // Parent arrives after its child.
        vec![middle.clone(), head.clone()],
    ] {
        let incoming = fragment(&range, commits.clone(), 1);
        assert_eq!(incoming.dependencies().collect::<Vec<_>>(), expected);
        let streamed = UnverifiedItem::verify_fragment(
            range.clone(),
            vec![evidence(1, *head.hash())],
            commits.into_iter().map(Ok),
        )
        .unwrap();
        let owned = incoming.verify().unwrap();
        for verified in [&owned, &streamed] {
            assert_eq!(
                verified.external_parents().copied().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                verified.dependencies().copied().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
                vec![*middle.hash(), *head.hash()]
            );
        }
        assert_eq!(owned.signature_boundary(), streamed.signature_boundary());
    }

    // A supplied member ceases to be external, even when it arrives last.
    // Admission, not stateless verification, detects that it is excluded history.
    let verified = UnverifiedItem::Fragment {
        range,
        checkpoints: BTreeSet::new(),
        commits: vec![head.clone(), middle, external],
        signatures: vec![evidence(1, *head.hash()), evidence(2, *head.hash())],
    }
    .verify()
    .unwrap();
    assert_eq!(
        verified.external_parents().copied().collect::<Vec<_>>(),
        vec![*boundary.hash()]
    );
}

#[test]
fn external_parent_at_or_above_boundary_depth_requires_a_declared_boundary() {
    for parent_depth in [1, 2] {
        let parent = at_depth(2, vec![], parent_depth);
        let head = at_depth(1, vec![*parent.hash()], 1);
        let range = FragmentRange {
            head: *head.hash(),
            boundary: BTreeSet::new(),
        };
        assert_eq!(
            fragment(&range, vec![head.clone()], 1)
                .verify()
                .unwrap_err(),
            VerificationError::InvalidItem
        );
        assert_eq!(
            UnverifiedItem::verify_fragment(range, vec![evidence(1, *head.hash())], [Ok(head)])
                .unwrap_err(),
            VerificationError::InvalidItem
        );
    }
}

#[test]
fn boundaries_discovered_only_through_excluded_history_are_allowed_statelessly() {
    let older_boundary = at_depth(2, vec![], 1);
    let excluded = at_depth(2, vec![*older_boundary.hash()], 0);
    let boundary = at_depth(2, vec![*excluded.hash()], 1);
    let head = at_depth(1, vec![*boundary.hash(), *excluded.hash()], 1);
    let verified = UnverifiedItem::Fragment {
        checkpoints: BTreeSet::new(),
        range: FragmentRange {
            head: *head.hash(),
            boundary: BTreeSet::from([*boundary.hash(), *older_boundary.hash()]),
        },
        commits: vec![head.clone()],
        signatures: vec![evidence(1, *head.hash())],
    }
    .verify()
    .unwrap();
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        BTreeSet::from([*boundary.hash(), *older_boundary.hash(), *excluded.hash()])
            .into_iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        verified.external_parents().copied().collect::<Vec<_>>(),
        BTreeSet::from([*boundary.hash(), *excluded.hash()])
            .into_iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(verified.commits().count(), 1);
    // Whether these dependencies actually occupy the claimed history is checked
    // only by graph admission; the verifier never reads their metadata.
}

#[test]
fn coverage_crosses_other_authors_and_order_is_lexicographic_topological() {
    let left = at_depth(1, vec![], 0);
    let right = at_depth(2, vec![], 0);
    let middle = at_depth(3, vec![*left.hash(), *right.hash()], 0);
    let head = at_depth(1, vec![*middle.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let mut records = vec![head.clone(), right.clone(), middle.clone(), left.clone()];
    let mut signatures = vec![
        evidence(1, *head.hash()),
        evidence(2, *head.hash()),
        evidence(3, *head.hash()),
        evidence(1, *left.hash()),
        evidence(4, *head.hash()),
    ];
    let mut expected = vec![*left.hash(), *right.hash()];
    expected.sort();
    expected.extend([*middle.hash(), *head.hash()]);
    for _ in 0..2 {
        let verified = UnverifiedItem::Fragment {
            range: range.clone(),
            checkpoints: BTreeSet::new(),
            commits: records.clone(),
            signatures: signatures.clone(),
        }
        .verify()
        .unwrap();
        assert_eq!(
            verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
            expected
        );
        records.reverse();
        signatures.reverse();
    }
}

#[test]
fn repeated_member_parents_do_not_create_a_false_cycle_or_duplicate_output() {
    let root = at_depth(1, vec![], 0);
    let middle = at_depth(1, vec![*root.hash(), *root.hash()], 0);
    let head = at_depth(1, vec![*middle.hash(), *middle.hash(), *root.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let verified = fragment(&range, vec![head.clone(), middle.clone(), root.clone()], 1)
        .verify()
        .unwrap();
    assert_eq!(
        verified.commits().map(|m| *m.hash()).collect::<Vec<_>>(),
        vec![*root.hash(), *middle.hash(), *head.hash()]
    );
    assert_eq!(
        verified.commits().nth(1).unwrap().parents(),
        middle.parents()
    );
    assert_eq!(verified.commits().nth(2).unwrap().parents(), head.parents());
}

#[test]
fn coverage_cannot_run_forward_or_sideways_between_members() {
    let left = at_depth(1, vec![], 0);
    let boundary = at_depth(2, vec![], 1);
    let right = at_depth(1, vec![*boundary.hash()], 0);
    let head = at_depth(2, vec![*left.hash(), *right.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*boundary.hash()]),
    };
    // Alice's signature on a sibling member cannot cover Alice's left root.
    assert_eq!(
        UnverifiedItem::Fragment {
            range,
            checkpoints: BTreeSet::new(),
            commits: vec![left.clone(), right.clone(), head.clone()],
            signatures: vec![evidence(1, *right.hash()), evidence(2, *head.hash())],
        }
        .verify()
        .unwrap_err(),
        VerificationError::MissingSignature {
            author: author(1),
            commit: *left.hash(),
        }
    );
    let same_author_head = at_depth(1, vec![*left.hash()], 1);
    assert_eq!(
        UnverifiedItem::Fragment {
            checkpoints: BTreeSet::new(),
            range: FragmentRange {
                head: *same_author_head.hash(),
                boundary: BTreeSet::new(),
            },
            commits: vec![left.clone(), same_author_head.clone()],
            signatures: vec![evidence(1, *left.hash())],
        }
        .verify()
        .unwrap_err(),
        VerificationError::MissingSignature {
            author: author(1),
            commit: *same_author_head.hash(),
        }
    );
}

#[test]
fn fragment_rejects_signature_targets_outside_its_members() {
    let root = at_depth(1, vec![], 0);
    let head = at_depth(1, vec![*root.hash()], 1);
    let later = at_depth(1, vec![*head.hash()], 0);
    assert_eq!(
        UnverifiedItem::Fragment {
            checkpoints: BTreeSet::new(),
            range: FragmentRange {
                head: *head.hash(),
                boundary: BTreeSet::new(),
            },
            commits: vec![head.clone(), root],
            signatures: vec![evidence(1, *head.hash()), evidence(1, *later.hash())],
        }
        .verify()
        .unwrap_err(),
        VerificationError::InvalidItem
    );
}

#[test]
fn higher_depth_heads_do_not_stop_at_lower_depth_checkpoints() {
    let root = at_depth(1, vec![], 0);
    let checkpoint = at_depth(1, vec![*root.hash()], 1);
    let head = at_depth(1, vec![*checkpoint.hash()], 2);
    let canonical = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    fragment(&canonical, vec![head.clone(), checkpoint.clone(), root], 1)
        .verify()
        .unwrap();
    let wrong_range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::from([*checkpoint.hash()]),
    };
    assert_eq!(
        fragment(&wrong_range, vec![head], 1).verify().unwrap_err(),
        VerificationError::InvalidItem
    );
}
